//! Process supervision helpers shared by the `run` shims and service
//! installation.

use std::path::PathBuf;

use anyhow::Result;

use crate::config::{default_admin_config_path, default_daemon_config_path, expand_tilde};
use crate::config_paths::{ensure_distinct_databases, Role, RolePaths};

pub mod health;
pub mod status;
pub mod supervisor;
pub mod systemd;

use crate::cli::ProcessMode;

/// How a lifecycle command will manage the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMode {
    Systemd,
    Supervisor,
}

/// Resolve the mode for stop/restart/status. A live supervisor state wins over
/// an installed unit so an explicitly supervised role can always be stopped.
pub fn active_mode(paths: &RolePaths, requested: ProcessMode) -> anyhow::Result<ActiveMode> {
    match requested {
        ProcessMode::Auto => {
            if supervisor_live(paths) {
                Ok(ActiveMode::Supervisor)
            } else {
                select_mode(
                    ProcessMode::Auto,
                    systemd::unit_installed(paths),
                    systemd::systemd_available(),
                )
            }
        }
        other => select_mode(
            other,
            systemd::unit_installed(paths),
            systemd::systemd_available(),
        ),
    }
}

pub async fn start_role(
    paths: &RolePaths,
    requested: ProcessMode,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    match select_mode(
        requested,
        systemd::unit_installed(paths),
        systemd::systemd_available(),
    )? {
        ActiveMode::Systemd => {
            if supervisor_live(paths) {
                anyhow::bail!(
                    "{} is already running under the fallback supervisor; stop it before starting systemd",
                    paths.role.as_str()
                );
            }
            systemd::start(paths)
        }
        ActiveMode::Supervisor => {
            supervisor::start(paths, std::time::Duration::from_secs(timeout_secs.max(1))).await
        }
    }
}

pub fn supervisor_live(paths: &RolePaths) -> bool {
    supervisor::read_state(&paths.supervisor_state()).is_some_and(|state| {
        supervisor::pid_alive(state.supervisor_pid)
            && supervisor::pid_matches(state.supervisor_pid, &["__supervise", paths.role.as_str()])
    })
}

pub async fn stop_role(
    paths: &RolePaths,
    requested: ProcessMode,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    match active_mode(paths, requested)? {
        ActiveMode::Systemd => systemd::stop(paths),
        ActiveMode::Supervisor => {
            supervisor::stop(paths, std::time::Duration::from_secs(timeout_secs.max(1))).await
        }
    }
}

pub async fn restart_role(
    paths: &RolePaths,
    requested: ProcessMode,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    match active_mode(paths, requested)? {
        ActiveMode::Systemd => systemd::restart(paths),
        ActiveMode::Supervisor => {
            let limit = std::time::Duration::from_secs(timeout_secs.max(1));
            supervisor::stop(paths, limit).await?;
            supervisor::start(paths, limit).await
        }
    }
}

/// Resolve `--mode` for `start`. Systemd requires an installed unit; auto
/// falls back to the supervisor when the unit or systemctl is unavailable.
pub fn select_mode(
    requested: ProcessMode,
    unit_installed: bool,
    systemd_available: bool,
) -> anyhow::Result<ActiveMode> {
    match requested {
        ProcessMode::Systemd if !unit_installed || !systemd_available => {
            anyhow::bail!(
                "systemd mode requested but the unit is not installed or systemctl is unavailable; run `fleet admin|daemon install-service` first"
            )
        }
        ProcessMode::Systemd => Ok(ActiveMode::Systemd),
        ProcessMode::Supervisor => Ok(ActiveMode::Supervisor),
        ProcessMode::Auto if unit_installed && systemd_available => Ok(ActiveMode::Systemd),
        ProcessMode::Auto => Ok(ActiveMode::Supervisor),
    }
}

/// Exec the role server in the foreground. Used by `fleet admin run` and
/// `fleet daemon run`, which are the `ExecStart` targets of the systemd units.
pub async fn run_role(paths: &RolePaths) -> Result<()> {
    check_database_separation(paths)?;
    if health::probe(paths).await {
        anyhow::bail!(
            "{} is already running; stop it or use `fleet {} restart`",
            paths.role.as_str(),
            paths.role.as_str()
        );
    }

    let binary = paths.require_binary()?;
    let mut command = std::process::Command::new(binary);
    command.arg("--config").arg(&paths.config).arg("serve");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = CommandExt::exec(&mut command);
        anyhow::bail!("failed to exec {}: {error}", binary.display());
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        if !status.success() {
            anyhow::bail!("{} exited with {status}", binary.display());
        }
        Ok(())
    }
}

/// Reject a configuration where the admin and daemon roles resolve to the same
/// SQLite database. The servers apply their own environment overrides, so the
/// check loads both configs through the same code path.
pub(crate) fn check_database_separation(paths: &RolePaths) -> Result<()> {
    let (admin_database, daemon_database) = match paths.role {
        Role::Admin => {
            let admin = fleet_core_config::AdminConfig::load(Some(&paths.config))?;
            let daemon = fleet_core_config::DaemonConfig::load(Some(&other_config_path(
                paths,
                Role::Daemon,
            )))?;
            (admin.storage.database, daemon.storage.database)
        }
        Role::Daemon => {
            let daemon = fleet_core_config::DaemonConfig::load(Some(&paths.config))?;
            let admin =
                fleet_core_config::AdminConfig::load(Some(&other_config_path(paths, Role::Admin)))?;
            (admin.storage.database, daemon.storage.database)
        }
    };
    ensure_distinct_databases(
        &expand_tilde(admin_database),
        &expand_tilde(daemon_database),
    )
}

/// The other role's config path. System installs keep both roles in
/// `/etc/fleet`; user installs use the documented `~/.fleet/<role>/config.toml`
/// defaults.
///
/// The system unit runs as a non-root service user without `--system`, so the
/// resolved scope is `User` even though the config lives in `/etc/fleet`. Detect
/// that layout from the config path as well.
fn other_config_path(paths: &RolePaths, other: Role) -> PathBuf {
    if paths.is_system() || paths.config.starts_with("/etc/fleet") {
        PathBuf::from("/etc/fleet").join(format!("{}.toml", other.as_str()))
    } else {
        match other {
            Role::Admin => default_admin_config_path(),
            Role::Daemon => default_daemon_config_path(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prefers_systemd_only_when_the_unit_is_installed() {
        assert_eq!(
            select_mode(ProcessMode::Auto, true, true).unwrap(),
            ActiveMode::Systemd
        );
        assert_eq!(
            select_mode(ProcessMode::Auto, false, true).unwrap(),
            ActiveMode::Supervisor
        );
        assert_eq!(
            select_mode(ProcessMode::Auto, true, false).unwrap(),
            ActiveMode::Supervisor
        );
    }

    #[test]
    fn explicit_systemd_requires_an_installed_unit() {
        let error = select_mode(ProcessMode::Systemd, false, true).unwrap_err();
        assert!(error.to_string().contains("install-service"), "{error}");
        assert_eq!(
            select_mode(ProcessMode::Supervisor, true, true).unwrap(),
            ActiveMode::Supervisor
        );
    }
}

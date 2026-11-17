//! Process supervision helpers shared by the `run` shims and service
//! installation.

use std::path::PathBuf;

use anyhow::Result;

use crate::config::{default_admin_config_path, default_daemon_config_path, expand_tilde};
use crate::config_paths::{ensure_distinct_databases, Role, RolePaths};

pub mod health;
pub mod launchd;
pub mod service;
pub mod status;
pub mod supervisor;
pub mod systemd;
pub mod windows_service;
pub mod windows_task;

use crate::cli::ProcessMode;

/// How a lifecycle command will manage the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMode {
    Service,
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
                    service::installed(paths),
                    service::available_for(paths).is_some(),
                )
            }
        }
        other => select_mode(
            other,
            service::installed(paths),
            service::available_for(paths).is_some(),
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
        service::installed(paths),
        service::available_for(paths).is_some(),
    )? {
        ActiveMode::Service => {
            if supervisor_live(paths) {
                anyhow::bail!(
                    "{} is already running under the fallback supervisor; stop it before starting the {} service",
                    paths.role.as_str(),
                    service::prompt_label_for(paths)
                );
            }
            service::start(paths)
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
        ActiveMode::Service => service::stop(paths),
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
        ActiveMode::Service => service::restart(paths),
        ActiveMode::Supervisor => {
            let limit = std::time::Duration::from_secs(timeout_secs.max(1));
            supervisor::stop(paths, limit).await?;
            supervisor::start(paths, limit).await
        }
    }
}

/// Resolve `--mode` for `start`. Service mode requires an installed native
/// service; auto falls back to the supervisor when the service or backend is
/// unavailable.
pub fn select_mode(
    requested: ProcessMode,
    unit_installed: bool,
    service_available: bool,
) -> anyhow::Result<ActiveMode> {
    match requested {
        ProcessMode::Service if !unit_installed || !service_available => {
            anyhow::bail!(
                "service mode requested but no native service is installed or available; run `fleet admin|daemon install-service` first"
            )
        }
        ProcessMode::Service => Ok(ActiveMode::Service),
        ProcessMode::Supervisor => Ok(ActiveMode::Supervisor),
        ProcessMode::Auto if unit_installed && service_available => Ok(ActiveMode::Service),
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

/// The other role's config path.
///
/// Fleet has a single server profile: both roles keep their configuration under
/// `~/.fleet/<role>/config.toml`.
fn other_config_path(_paths: &RolePaths, other: Role) -> PathBuf {
    match other {
        Role::Admin => default_admin_config_path(),
        Role::Daemon => default_daemon_config_path(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prefers_the_native_service_only_when_installed() {
        assert_eq!(
            select_mode(ProcessMode::Auto, true, true).unwrap(),
            ActiveMode::Service
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
    fn explicit_service_requires_an_installed_service() {
        let error = select_mode(ProcessMode::Service, false, true).unwrap_err();
        assert!(error.to_string().contains("install-service"), "{error}");
        assert_eq!(
            select_mode(ProcessMode::Supervisor, true, true).unwrap(),
            ActiveMode::Supervisor
        );
    }
}

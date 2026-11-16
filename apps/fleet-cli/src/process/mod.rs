//! Process supervision helpers shared by the `run` shims and service
//! installation.

use std::path::PathBuf;

use anyhow::Result;

use crate::config::{default_admin_config_path, default_daemon_config_path, expand_tilde};
use crate::config_paths::{ensure_distinct_databases, Role, RolePaths};

pub mod systemd;

/// Exec the role server in the foreground. Used by `fleet admin run` and
/// `fleet daemon run`, which are the `ExecStart` targets of the systemd units.
pub fn run_role(paths: &RolePaths) -> Result<()> {
    check_database_separation(paths)?;

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
fn check_database_separation(paths: &RolePaths) -> Result<()> {
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

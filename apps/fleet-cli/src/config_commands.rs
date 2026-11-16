//! `fleet admin config` and `fleet daemon config`.
//!
//! These commands inspect service configuration with [`RoleArgs::config_paths`]
//! semantics: they never discover or require the `fleet-admin`/`fleet-daemon`
//! server binaries, and they never call `resolve_paths`, which would create
//! directories. Display paths are expanded with [`expand_tilde`].
//!
//! [`RoleArgs::config_paths`]: crate::cli::RoleArgs::config_paths

use std::path::Path;

use anyhow::{Context, Result};
use fleet_core_auth::machine_token_path;
use fleet_core_config::{AdminConfig, DaemonConfig};

use crate::config::expand_tilde;
use crate::config_paths::{ensure_distinct_databases, RolePaths};

/// `fleet admin config show [--reveal]`.
pub fn admin_show(paths: &RolePaths, reveal: bool, json: bool) -> Result<()> {
    let config = AdminConfig::load(Some(&paths.config))
        .with_context(|| format!("invalid admin config {}", paths.config.display()))?;
    let config_path = expand_tilde(paths.config.clone());
    let database = expand_tilde(config.storage.database.clone());
    let token = match config.auth.admin_token.as_str() {
        "" => "<none>".to_string(),
        token if reveal => token.to_string(),
        _ => "<redacted>".to_string(),
    };
    let bind = format!("{}:{}", config.server.host, config.server.port);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "config": config_path,
                "database": database,
                "bind": bind,
                "public_url": config.server.public_url,
                "admin_token": token,
            }))?
        );
    } else {
        println!("config    {}", config_path.display());
        println!("database  {}", database.display());
        println!("bind      {bind}");
        if let Some(public_url) = &config.server.public_url {
            println!("public    {public_url}");
        }
        println!("token     {token}");
    }
    Ok(())
}

/// `fleet admin config path`.
pub fn admin_path(paths: &RolePaths) -> Result<()> {
    println!("{}", expand_tilde(paths.config.clone()).display());
    Ok(())
}

/// `fleet admin config validate`.
///
/// Validates the admin TOML and rejects a database shared with the daemon.
/// `daemon_config` comes from the CLI config (or the documented default).
pub fn admin_validate(paths: &RolePaths, daemon_config: &Path) -> Result<()> {
    let admin = AdminConfig::load(Some(&paths.config))
        .with_context(|| format!("invalid admin config {}", paths.config.display()))?;
    let daemon = DaemonConfig::load(Some(daemon_config))
        .with_context(|| format!("invalid daemon config {}", daemon_config.display()))?;
    ensure_distinct_databases(
        &expand_tilde(admin.storage.database.clone()),
        &expand_tilde(daemon.storage.database.clone()),
    )?;
    println!(
        "config {} is valid",
        expand_tilde(paths.config.clone()).display()
    );
    Ok(())
}

/// `fleet daemon config show`.
pub fn daemon_show(paths: &RolePaths, json: bool) -> Result<()> {
    let config = DaemonConfig::load(Some(&paths.config))
        .with_context(|| format!("invalid daemon config {}", paths.config.display()))?;
    let config_path = expand_tilde(paths.config.clone());
    let database = expand_tilde(config.storage.database.clone());
    let key_file = expand_tilde(config.machine.key_file.clone());
    let paired = machine_token_path(&key_file).exists();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "config": config_path,
                "database": database,
                "key_file": key_file,
                "server_url": config.connection.server_url,
                "machine_id": config.machine.id,
                "machine_name": config.machine.name,
                "paired": paired,
            }))?
        );
    } else {
        println!("config    {}", config_path.display());
        println!("database  {}", database.display());
        println!("key file  {}", key_file.display());
        if let Some(server_url) = &config.connection.server_url {
            println!("server    {server_url}");
        }
        if let Some(machine_id) = &config.machine.id {
            println!("machine   {machine_id}");
        }
        if let Some(name) = &config.machine.name {
            println!("name      {name}");
        }
        println!("paired    {}", if paired { "yes" } else { "no" });
    }
    Ok(())
}

/// `fleet daemon config path`.
pub fn daemon_path(paths: &RolePaths) -> Result<()> {
    println!("{}", expand_tilde(paths.config.clone()).display());
    Ok(())
}

/// `fleet daemon config validate`.
///
/// Validates the daemon TOML and rejects a database shared with the admin.
/// `admin_config` comes from the CLI config (or the documented default).
pub fn daemon_validate(paths: &RolePaths, admin_config: &Path) -> Result<()> {
    let daemon = DaemonConfig::load(Some(&paths.config))
        .with_context(|| format!("invalid daemon config {}", paths.config.display()))?;
    let admin = AdminConfig::load(Some(admin_config))
        .with_context(|| format!("invalid admin config {}", admin_config.display()))?;
    ensure_distinct_databases(
        &expand_tilde(admin.storage.database.clone()),
        &expand_tilde(daemon.storage.database.clone()),
    )?;
    println!(
        "config {} is valid",
        expand_tilde(paths.config.clone()).display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::*;
    use crate::config_paths::{Role, Scope};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_VARS: &[&str] = &[
        "FLEET_DB",
        "FLEET_DATA_DIR",
        "FLEET_ADMIN_TOKEN",
        "FLEET_MACHINE_NAME",
        "FLEET_SERVER_URL",
        "FLEET_PAIR_CODE",
        "FLEET_BIND",
        "FLEET_GIT_WRITES",
        "FLEET_CONFIG",
    ];

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        for key in ENV_VARS {
            std::env::remove_var(key);
        }
        guard
    }

    fn role_paths(role: Role, config: PathBuf) -> RolePaths {
        let state_dir = config.parent().unwrap().to_path_buf();
        RolePaths {
            role,
            scope: Scope::User,
            config,
            state_dir: state_dir.clone(),
            database: state_dir.join("role.db"),
            binary: None,
            fleet_binary: PathBuf::from("/usr/local/bin/fleet"),
            service_user: "tester".to_string(),
            service_group: "tester".to_string(),
            env_file: None,
            web_dir: None,
        }
    }

    fn write_admin(path: &Path, database: &Path) {
        std::fs::write(
            path,
            format!(
                "[storage]\ndatabase = \"{}\"\n\n[server]\nhost = \"127.0.0.1\"\nport = 8787\n",
                database.display()
            ),
        )
        .unwrap();
    }

    fn write_daemon(path: &Path, database: &Path, key_file: &Path) {
        std::fs::write(
            path,
            format!(
                "[machine]\nkey_file = \"{}\"\n\n[storage]\ndatabase = \"{}\"\n",
                key_file.display(),
                database.display()
            ),
        )
        .unwrap();
    }

    #[test]
    fn show_path_and_validate_use_config_paths() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let admin_config = dir.path().join("admin.toml");
        let daemon_config = dir.path().join("daemon.toml");
        write_admin(&admin_config, &dir.path().join("admin.db"));
        write_daemon(
            &daemon_config,
            &dir.path().join("daemon.db"),
            &dir.path().join("machine.key"),
        );

        let admin = role_paths(Role::Admin, admin_config.clone());
        let daemon = role_paths(Role::Daemon, daemon_config.clone());

        admin_show(&admin, false, false).unwrap();
        admin_show(&admin, false, true).unwrap();
        admin_path(&admin).unwrap();
        admin_validate(&admin, &daemon_config).unwrap();

        daemon_show(&daemon, false).unwrap();
        daemon_show(&daemon, true).unwrap();
        daemon_path(&daemon).unwrap();
        daemon_validate(&daemon, &admin_config).unwrap();

        // Config commands never create identity material.
        assert!(!dir.path().join("machine.key").exists());
        assert!(!dir.path().join("machine.key.id").exists());
        assert!(!dir.path().join("machine.key.token").exists());
    }

    #[test]
    fn validate_rejects_a_shared_database() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.db");
        let admin_config = dir.path().join("admin.toml");
        let daemon_config = dir.path().join("daemon.toml");
        write_admin(&admin_config, &shared);
        write_daemon(&daemon_config, &shared, &dir.path().join("machine.key"));

        let admin = role_paths(Role::Admin, admin_config);
        let daemon = role_paths(Role::Daemon, daemon_config);

        let error = admin_validate(&admin, &daemon.config).expect_err("shared database");
        assert!(error.to_string().contains("must be different"), "{error}");
        let error = daemon_validate(&daemon, &admin.config).expect_err("shared database");
        assert!(error.to_string().contains("must be different"), "{error}");
    }
}

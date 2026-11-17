//! Local `fleet-daemon` commands and read-only machine identity.
//!
//! `fleet daemon providers`, `workspace ...`, and `rotate-key` delegate to the
//! resolved `fleet-daemon` binary. `identity show` is implemented here so it
//! works without that binary and never creates key, id, or token files.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use fleet_core_auth::{machine_id_path, machine_token_path, MachineIdentity};
use fleet_core_config::DaemonConfig;
use serde_json::Value;

use crate::config::expand_tilde;
use crate::config_paths::RolePaths;

/// A `fleet-daemon` invocation with `--config <path>` placed before the
/// subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl DaemonCommand {
    pub fn new(paths: &RolePaths, args: Vec<String>) -> Result<Self> {
        let program = paths.require_binary()?.to_path_buf();
        let mut full = Vec::with_capacity(args.len() + 2);
        full.push("--config".to_string());
        full.push(paths.config.display().to_string());
        full.extend(args);
        Ok(Self {
            program,
            args: full,
        })
    }
}

/// Run a `fleet-daemon` subcommand with inherited stdio.
pub async fn run_daemon(args: &[String], paths: &RolePaths) -> Result<()> {
    let command = DaemonCommand::new(paths, args.to_vec())?;
    let status = tokio::process::Command::new(&command.program)
        .args(&command.args)
        .status()
        .await
        .with_context(|| format!("failed to run {}", command.program.display()))?;
    if !status.success() {
        bail!("{} exited with {}", command.program.display(), status);
    }
    Ok(())
}

/// Run a `fleet-daemon` subcommand that prints JSON and parse the result.
#[allow(dead_code)] // available for local commands that need structured output
pub async fn run_daemon_json(paths: &RolePaths, args: &[String]) -> Result<Value> {
    let command = DaemonCommand::new(paths, args.to_vec())?;
    let output = tokio::process::Command::new(&command.program)
        .args(&command.args)
        .output()
        .await
        .with_context(|| format!("failed to run {}", command.program.display()))?;
    if !output.status.success() {
        bail!(
            "{} exited with {}: {}",
            command.program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{} returned invalid JSON", command.program.display()))
}

/// Read machine identity without creating key, id, or token files.
///
/// The daemon config is loaded as-is: `resolve_paths` is deliberately not
/// called because it creates parent directories.
pub fn identity_json(paths: &RolePaths) -> Result<Value> {
    let config = DaemonConfig::load(Some(&paths.config))
        .with_context(|| format!("invalid daemon config {}", paths.config.display()))?;
    let key_file = expand_tilde(config.machine.key_file.clone());
    let fingerprint = if key_file.exists() {
        Some(MachineIdentity::load(&key_file)?.fingerprint())
    } else {
        None
    };
    let id_path = machine_id_path(&key_file);
    let machine_id = std::fs::read_to_string(&id_path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let token_file = machine_token_path(&key_file);

    Ok(serde_json::json!({
        "machine_id": machine_id,
        "machine_name": config.machine.name,
        "key_file": key_file,
        "fingerprint": fingerprint,
        "token_file": token_file,
        "paired": token_file.exists(),
    }))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::*;
    use crate::config_paths::Role;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_VARS: &[&str] = &[
        "FLEET_DB",
        "FLEET_DATA_DIR",
        "FLEET_MACHINE_NAME",
        "FLEET_SERVER_URL",
        "FLEET_PAIR_CODE",
        "FLEET_BIND",
        "FLEET_GIT_WRITES",
        "FLEET_ADMIN_TOKEN",
        "FLEET_CONFIG",
    ];

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        for key in ENV_VARS {
            std::env::remove_var(key);
        }
        guard
    }

    fn test_paths(dir: &Path, config: PathBuf, binary: Option<PathBuf>) -> RolePaths {
        RolePaths {
            role: Role::Daemon,
            config,
            state_dir: dir.join("state"),
            database: dir.join("state/fleet-daemon.db"),
            binary,
            fleet_binary: PathBuf::from("/usr/local/bin/fleet"),
            service_user: "tester".to_string(),
            service_group: "tester".to_string(),
            web_dir: None,
        }
    }

    #[test]
    fn daemon_command_puts_config_first() {
        let dir = tempfile::tempdir().unwrap();
        let binary = std::env::current_exe().unwrap();
        let config = dir.path().join("daemon/config.toml");
        let paths = test_paths(dir.path(), config.clone(), Some(binary.clone()));

        let command =
            DaemonCommand::new(&paths, vec!["providers".to_string(), "--extra".to_string()])
                .unwrap();

        assert_eq!(command.program, binary);
        assert_eq!(
            command.args,
            vec![
                "--config".to_string(),
                config.display().to_string(),
                "providers".to_string(),
                "--extra".to_string(),
            ]
        );
    }

    #[test]
    fn daemon_command_requires_a_binary() {
        let dir = tempfile::tempdir().unwrap();
        let paths = test_paths(dir.path(), dir.path().join("daemon.toml"), None);
        let error = DaemonCommand::new(&paths, vec!["providers".to_string()]).unwrap_err();
        assert!(error.to_string().contains("fleet-daemon"), "{error}");
    }

    #[test]
    fn identity_json_for_a_missing_key_is_read_only() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("machine.key");
        let config = dir.path().join("daemon.toml");
        std::fs::write(
            &config,
            format!(
                "[machine]\nkey_file = \"{}\"\n\n[storage]\ndatabase = \"{}\"\n",
                key_file.display(),
                dir.path().join("fleet-daemon.db").display()
            ),
        )
        .unwrap();
        let before = std::fs::read(&config).unwrap();

        let paths = test_paths(dir.path(), config.clone(), None);
        let value = identity_json(&paths).unwrap();

        assert!(value["fingerprint"].is_null());
        assert!(value["machine_id"].is_null());
        assert_eq!(value["paired"], serde_json::json!(false));
        assert!(!key_file.exists());
        assert!(!machine_id_path(&key_file).exists());
        assert!(!machine_token_path(&key_file).exists());
        assert_eq!(std::fs::read(&config).unwrap(), before);
    }
}

//! `fleet-daemon` configuration, pairing and process lifecycle.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fleet_core_protocol::machine::{Machine, MachineStatus};
use serde_json::Value;

use super::admin::Admin;
use super::env::TestEnv;
use super::ports::free_port;
use super::process::ManagedProcess;
use super::wait::{wait_until, DEFAULT_TIMEOUT};

/// Tuning knobs for a daemon under test.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    pub heartbeat_interval_ms: u64,
    pub reconnect_max_ms: u64,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            heartbeat_interval_ms: 500,
            reconnect_max_ms: 1_000,
        }
    }
}

/// One daemon's configuration and process state.
pub struct Daemon {
    pub name: String,
    pub port: u16,
    pub config: PathBuf,
    pub db: PathBuf,
    pub key_file: PathBuf,
    pub options: DaemonOptions,
    process: Option<ManagedProcess>,
}

impl Daemon {
    /// Write an unpaired daemon configuration pointing at `admin`.
    pub fn new(env: &TestEnv, admin: &Admin, name: &str, options: DaemonOptions) -> Result<Self> {
        let slug = slug(name);
        let dir = env.path().join("daemons").join(&slug);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create daemon directory {}", dir.display()))?;
        let port = free_port();
        let config = dir.join("daemon.toml");
        let db = dir.join("fleet-daemon.db");
        let key_file = dir.join("machine.key");
        let daemon = Self {
            name: name.to_owned(),
            port,
            config,
            db,
            key_file,
            options,
            process: None,
        };
        daemon.write_config(&admin.ws_url())?;
        Ok(daemon)
    }

    pub fn write_config(&self, server_url: &str) -> Result<()> {
        let toml = format!(
            "[machine]\n\
             name = \"{name}\"\n\
             key_file = \"{key_file}\"\n\
             \n\
             [daemon]\n\
             host = \"127.0.0.1\"\n\
             port = {port}\n\
             \n\
             [storage]\n\
             database = \"{db}\"\n\
             \n\
             [agent]\n\
             default_provider = \"mock\"\n\
             \n\
             [connection]\n\
             server_url = \"{server_url}\"\n\
             heartbeat_interval_ms = {heartbeat}\n\
             reconnect_max_ms = {reconnect}\n",
            name = self.name,
            key_file = self.key_file.display(),
            port = self.port,
            db = self.db.display(),
            server_url = server_url,
            heartbeat = self.options.heartbeat_interval_ms,
            reconnect = self.options.reconnect_max_ms,
        );
        std::fs::write(&self.config, toml)
            .with_context(|| format!("write {}", self.config.display()))
    }

    /// Run `fleet daemon setup` against a live admin, writing the machine
    /// token next to the key file.
    pub async fn pair(&self, env: &TestEnv, admin: &Admin) -> Result<()> {
        let output = env
            .fleet()
            .args([
                "--admin-url",
                &admin.url,
                "--admin-token",
                &admin.token,
                "daemon",
                "setup",
                "--no-install-service",
                "--org",
                "org_default",
                "--server",
                &admin.ws_url(),
                "--config",
                self.config.to_str().expect("config path is UTF-8"),
                "--name",
                &self.name,
            ])
            .output()
            .await
            .context("run fleet daemon setup")?;
        if !output.status.success() {
            bail!(
                "pairing {} failed ({})\nstdout:\n{}\nstderr:\n{}",
                self.name,
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        Ok(())
    }

    pub fn token_path(&self) -> PathBuf {
        let mut name = self.key_file.clone().into_os_string();
        name.push(".token");
        PathBuf::from(name)
    }

    pub fn log_path(&self) -> PathBuf {
        self.config
            .parent()
            .expect("daemon config has a parent")
            .join("daemon.log")
    }

    pub fn logs(&self) -> String {
        std::fs::read_to_string(self.log_path()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.process.is_some()
    }

    pub async fn start(&mut self, env: &TestEnv) -> Result<()> {
        if self.process.is_some() {
            return Ok(());
        }
        let mut command = env.command(&env.bins.daemon);
        command
            .arg("--config")
            .arg(&self.config)
            .arg("serve")
            .env("RUST_LOG", "fleet_daemon=info,fleet_core_connection=info");
        let process = ManagedProcess::spawn(
            format!("fleet-daemon:{}", self.name),
            &mut command,
            self.log_path(),
        )?;
        self.process = Some(process);
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(mut process) = self.process.take() {
            process.shutdown().await?;
        }
        Ok(())
    }

    pub async fn restart(&mut self, env: &TestEnv) -> Result<()> {
        self.stop().await?;
        self.start(env).await
    }

    /// Kill the daemon without a graceful shutdown, simulating a crash.
    pub async fn crash(&mut self) -> Result<()> {
        if let Some(mut process) = self.process.take() {
            process.kill().await?;
        }
        Ok(())
    }

    /// Wait until the admin reports this machine as online.
    pub async fn wait_online(&self, admin: &Admin) -> Result<Machine> {
        let api = admin.api();
        let name = self.name.clone();
        wait_until(
            &format!("machine {name} to come online"),
            DEFAULT_TIMEOUT,
            || {
                let api = api.clone();
                let name = name.clone();
                async move {
                    let machines = api.machines().await?;
                    match machines.into_iter().find(|machine| machine.name == name) {
                        Some(machine) if machine.status == MachineStatus::Online => Ok(machine),
                        Some(machine) => {
                            Err(anyhow::anyhow!("machine {name} is {:?}", machine.status))
                        }
                        None => Err(anyhow::anyhow!("machine {name} is not registered")),
                    }
                }
            },
        )
        .await
    }

    /// Rotate the machine key and return the JSON summary printed by the CLI.
    pub async fn rotate_key(&self, env: &TestEnv) -> Result<Value> {
        let output = env
            .command(&env.bins.daemon)
            .args([
                "--config",
                self.config.to_str().expect("config path is UTF-8"),
                "rotate-key",
            ])
            .output()
            .await
            .context("run fleet-daemon rotate-key")?;
        if !output.status.success() {
            bail!(
                "rotate-key failed ({})\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        parse_json_output(&output.stdout)
    }

    pub fn previous_key_path(&self) -> PathBuf {
        let mut name = self.key_file.clone().into_os_string();
        name.push(".previous");
        PathBuf::from(name)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // `ManagedProcess` already kills on drop; this exists so the field is
        // explicitly used and daemons always stop before their workspace.
        let _ = self.process.take();
    }
}

fn slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    if slug.is_empty() {
        "daemon".to_owned()
    } else {
        slug
    }
}

fn parse_json_output(stdout: &[u8]) -> Result<Value> {
    let text = String::from_utf8_lossy(stdout);
    if let Ok(value) = serde_json::from_str(text.trim()) {
        return Ok(value);
    }
    for line in text.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(value) = serde_json::from_str(line) {
                return Ok(value);
            }
        }
    }
    bail!("could not parse JSON from command output:\n{text}")
}

/// Whether a config file contains an inline machine token.
pub fn config_has_token(path: &Path) -> Result<bool> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(text.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("token") && line.contains('=')
    }))
}

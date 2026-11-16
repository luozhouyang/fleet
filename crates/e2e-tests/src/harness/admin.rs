//! `fleet-admin` process lifecycle.

use std::path::PathBuf;

use anyhow::{bail, Result};

use super::env::TestEnv;
use super::ports::free_port;
use super::process::ManagedProcess;
use super::wait::{wait_until, DEFAULT_TIMEOUT};
use crate::api::Api;

/// Options for starting the control plane.
#[derive(Debug, Default)]
pub struct AdminOptions {
    /// Fixed admin token; a random one is generated when omitted.
    pub token: Option<String>,
    /// Enables Git writes behind a dedicated `X-Fleet-Git-Token`.
    pub git_write_token: Option<String>,
    /// Static web assets to serve.
    pub web_dir: Option<PathBuf>,
}

/// A running control plane.
pub struct Admin {
    pub port: u16,
    pub token: String,
    pub db: PathBuf,
    pub url: String,
    pub git_write_token: Option<String>,
    process: ManagedProcess,
}

impl Admin {
    pub async fn start(env: &TestEnv, options: AdminOptions) -> Result<Self> {
        let port = free_port();
        let db = env.data_dir().join(format!("fleet-admin-{port}.db"));
        let token = options
            .token
            .unwrap_or_else(|| format!("e2e-admin-token-{port}"));
        let url = format!("http://127.0.0.1:{port}");

        let mut command = env.command(&env.bins.admin);
        command
            .arg("serve")
            .arg("--bind")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--db")
            .arg(&db)
            .arg("--admin-token")
            .arg(&token)
            .env("RUST_LOG", "fleet_admin=info");
        if let Some(web_dir) = &options.web_dir {
            command.arg("--web-dir").arg(web_dir);
        }
        if let Some(write_token) = &options.git_write_token {
            command.env("FLEET_GIT_WRITE_TOKEN", write_token);
        }

        let log_path = env.logs_dir().join(format!("fleet-admin-{port}.log"));
        let process = ManagedProcess::spawn(format!("fleet-admin:{port}"), &mut command, log_path)?;

        if let Err(error) = wait_for_health(&url, &token).await {
            bail!(
                "fleet-admin did not become healthy: {error}\n--- fleet-admin log ---\n{}",
                process.tail(80)
            );
        }

        Ok(Self {
            port,
            token,
            db,
            url,
            git_write_token: options.git_write_token,
            process,
        })
    }

    pub fn api(&self) -> Api {
        Api::new(&self.url, &self.token)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/ws/machine", self.port)
    }

    pub fn log_path(&self) -> std::path::PathBuf {
        self.process.log_path().to_path_buf()
    }

    pub fn logs(&self) -> String {
        self.process.logs()
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.process.shutdown().await
    }
}

async fn wait_for_health(url: &str, token: &str) -> Result<()> {
    let api = Api::new(url, token);
    wait_until("fleet-admin /api/v1/health", DEFAULT_TIMEOUT, || {
        let api = api.clone();
        async move {
            let health = api.get_value("/api/v1/health").await?;
            if health["ok"] == true {
                Ok(())
            } else {
                Err(anyhow::anyhow!("unexpected health payload: {health}"))
            }
        }
    })
    .await
}

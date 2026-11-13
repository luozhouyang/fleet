//! Spawning and connecting to an OpenCode server.

use std::time::Duration;

use agent_fleet_process_manager::{CommandSpec, ManagedProcess, ProcessEvent};

use crate::error::{Result, RuntimeError};

/// A private OpenCode server started by the daemon.
///
/// The child belongs to this value: dropping it terminates the server process
/// group (handled by the process manager).
pub struct OpenCodeServer {
    process: Option<ManagedProcess>,
    base_url: String,
    password: String,
}

impl OpenCodeServer {
    /// Spawn `opencode serve` on a random loopback port and wait for its banner.
    pub async fn spawn(bin: &str) -> Result<Self> {
        let spec = CommandSpec::new(bin).args(["serve", "--hostname", "127.0.0.1", "--port", "0"]);

        let process = ManagedProcess::spawn(spec).await.map_err(|err| {
            RuntimeError::ProviderUnavailable(format!("failed to spawn opencode server: {err}"))
        })?;
        let mut events = process.subscribe();

        let mut buffer = String::new();
        let mut url: Option<String> = None;
        let mut password: Option<String> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);

        while url.is_none() || password.is_none() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(RuntimeError::Timeout(
                    "opencode server did not print its listening banner".into(),
                ));
            }
            match tokio::time::timeout(remaining, events.recv()).await {
                Ok(Ok(ProcessEvent::Stdout(chunk))) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(position) = buffer.find('\n') {
                        let line: String = buffer.drain(..=position).collect();
                        let line = line.trim();
                        if let Some(rest) = line.strip_prefix("server listening on ") {
                            url = Some(rest.trim().to_string());
                        } else if let Some(rest) = line.strip_prefix("server password ") {
                            password = Some(rest.trim().to_string());
                        }
                    }
                }
                Ok(Ok(ProcessEvent::Stderr(chunk))) => {
                    tracing::debug!(
                        chunk = %String::from_utf8_lossy(&chunk),
                        "opencode server stderr"
                    );
                }
                Ok(Ok(ProcessEvent::Exited { code, .. })) => {
                    return Err(RuntimeError::ProviderUnavailable(format!(
                        "opencode server exited during startup (code {code:?})"
                    )));
                }
                Ok(Ok(ProcessEvent::Started { .. })) => {}
                Ok(Err(err)) => {
                    return Err(RuntimeError::ProviderUnavailable(format!(
                        "opencode server output channel closed: {err}"
                    )));
                }
                Err(_) => {
                    return Err(RuntimeError::Timeout(
                        "opencode server did not print its listening banner".into(),
                    ));
                }
            }
        }

        let url = url.expect("checked above");
        let password = password.expect("checked above");
        tracing::info!(url = %url, "opencode server started");
        Ok(Self {
            process: Some(process),
            base_url: format!("{}/api", url.trim_end_matches('/')),
            password,
        })
    }

    /// Create a provider endpoint for an already running server.
    pub fn connect(base_url: impl Into<String>, password: Option<String>) -> Self {
        let base = base_url.into();
        let base = base.trim_end_matches('/');
        let base = if base.ends_with("/api") {
            base.to_string()
        } else {
            format!("{base}/api")
        };
        Self {
            process: None,
            base_url: base,
            password: password.unwrap_or_default(),
        }
    }

    /// API base URL including the `/api` suffix.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Server password printed by `opencode serve`.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Whether a spawned server process is still running. External servers
    /// always report `true` because their lifecycle is managed elsewhere.
    pub fn is_running(&self) -> bool {
        match &self.process {
            Some(process) => process.is_running(),
            None => true,
        }
    }
}

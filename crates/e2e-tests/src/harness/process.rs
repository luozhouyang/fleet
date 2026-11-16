//! Child-process lifecycle with log capture and best-effort cleanup.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::{Child, Command};

/// A spawned service with its stdout/stderr redirected to one log file.
///
/// The process is killed on drop so a panicking test cannot leak servers.
/// When the test thread is panicking the last log lines are printed as well.
pub struct ManagedProcess {
    name: String,
    child: Option<Child>,
    log_path: PathBuf,
}

impl ManagedProcess {
    /// Spawn `command`, redirecting both streams to `log_path`.
    pub fn spawn(
        name: impl Into<String>,
        command: &mut Command,
        log_path: impl Into<PathBuf>,
    ) -> Result<Self> {
        let log_path = log_path.into();
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create log directory {}", parent.display()))?;
        }
        let name = name.into();
        let stdout = std::fs::File::create(&log_path)
            .with_context(|| format!("create {}", log_path.display()))?;
        let stderr = stdout
            .try_clone()
            .with_context(|| format!("clone {}", log_path.display()))?;
        command
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        let child = command.spawn().with_context(|| format!("spawn {name}"))?;
        Ok(Self {
            name,
            child: Some(child),
            log_path,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn logs(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    /// Last `lines` lines of the captured log.
    pub fn tail(&self, lines: usize) -> String {
        let logs = self.logs();
        let mut tail: Vec<&str> = logs.lines().rev().take(lines).collect();
        tail.reverse();
        tail.join("\n")
    }

    /// Ask the process to exit, escalating to SIGKILL after a short grace
    /// period.
    pub async fn shutdown(&mut self) -> Result<()> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            let _ = std::process::Command::new("kill")
                .args(["-INT", &pid.to_string()])
                .status();
        }
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(status) => {
                status.with_context(|| format!("wait for {}", self.name))?;
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            }
        }
        Ok(())
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        if std::thread::panicking() {
            eprintln!(
                "--- {} log ({}) ---\n{}",
                self.name,
                self.log_path.display(),
                self.tail(100)
            );
        }
    }
}

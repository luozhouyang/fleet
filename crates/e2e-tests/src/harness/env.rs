//! Isolated test environment: temporary home, CLI config and binaries.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio::process::Command;

use super::bin::Bins;

/// Environment variables that would leak the developer's real fleet state
/// into a test process.
const AMBIENT_FLEET_VARS: &[&str] = &[
    "FLEET_ADMIN_TOKEN",
    "FLEET_ADMIN_URL",
    "FLEET_ADMIN_BIN",
    "FLEET_BIND",
    "FLEET_CLI_CONFIG",
    "FLEET_CONFIG",
    "FLEET_DATA_DIR",
    "FLEET_DAEMON_BIN",
    "FLEET_DB",
    "FLEET_GIT_WRITES",
    "FLEET_GIT_WRITE_TOKEN",
    "FLEET_MACHINE_NAME",
    "FLEET_METRICS_ENABLED",
    "FLEET_METRICS_HOST",
    "FLEET_METRICS_PORT",
    "FLEET_PAIR_CODE",
    "FLEET_SERVER_URL",
    "FLEET_WEB_DIR",
];

/// One test's hermetic filesystem and process factory.
pub struct TestEnv {
    root: PathBuf,
    dir: Option<TempDir>,
    pub home: PathBuf,
    pub cli_config: PathBuf,
    pub bins: Bins,
    completed: Cell<bool>,
}

impl TestEnv {
    pub fn new() -> Result<Self> {
        let root = super::bin::workspace_root();
        let bins = Bins::locate().context(
            "the E2E binaries are not built; `pnpm e2e` builds them before running tests",
        )?;
        let dir = tempfile::Builder::new()
            .prefix("fleet-e2e-")
            .tempdir()
            .context("create the E2E temporary directory")?;
        let home = dir.path().join("home");
        let cli_config = home.join(".fleet").join("cli.toml");
        std::fs::create_dir_all(cli_config.parent().expect("cli config has a parent"))?;
        Ok(Self {
            root,
            dir: Some(dir),
            home,
            cli_config,
            bins,
            completed: Cell::new(false),
        })
    }

    /// Mark the test as successful so its temporary state is cleaned up.
    ///
    /// Failing tests never call this, so the directory and logs stay on disk
    /// for inspection.
    pub fn finish(&self) {
        self.completed.set(true);
    }

    pub fn completed(&self) -> bool {
        self.completed.get()
    }

    pub fn path(&self) -> &Path {
        self.dir.as_ref().expect("test environment is alive").path()
    }

    pub fn workspace_root(&self) -> &Path {
        &self.root
    }

    pub fn data_dir(&self) -> PathBuf {
        let dir = self.path().join("data");
        std::fs::create_dir_all(&dir).expect("create the data directory");
        dir
    }

    pub fn logs_dir(&self) -> PathBuf {
        let dir = self.path().join("logs");
        std::fs::create_dir_all(&dir).expect("create the logs directory");
        dir
    }

    /// Build a command with an isolated HOME and CLI configuration.
    pub fn command(&self, binary: &Path) -> Command {
        let mut command = Command::new(binary);
        command
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("FLEET_CLI_CONFIG", &self.cli_config)
            .env("NO_COLOR", "1");
        for name in AMBIENT_FLEET_VARS {
            command.env_remove(name);
        }
        // E2E tests start several isolated admins in parallel. The admin
        // metrics listener has a fixed default port, so leave it disabled in
        // spawned processes; the metrics endpoints have their own unit tests.
        command.env("FLEET_METRICS_ENABLED", "false");
        command
    }

    pub fn fleet(&self) -> Command {
        self.command(&self.bins.fleet)
    }

    /// Run a binary to completion and return its output.
    pub async fn run(&self, binary: &Path, args: &[&str]) -> Result<std::process::Output> {
        self.command(binary)
            .args(args)
            .output()
            .await
            .with_context(|| format!("run {}", binary.display()))
    }

    /// Run `fleet` and fail with combined output when the exit status is not
    /// successful.
    pub async fn fleet_ok(&self, args: &[&str]) -> Result<String> {
        let output = self.fleet().args(args).output().await?;
        if !output.status.success() {
            anyhow::bail!(
                "fleet {:?} failed ({})\nstdout:\n{}\nstderr:\n{}",
                args,
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Spawn a long-running binary with output captured under `logs/`.
    pub fn spawn(
        &self,
        binary: &Path,
        args: &[&str],
        log_name: &str,
    ) -> Result<super::process::ManagedProcess> {
        let mut command = self.command(binary);
        command.args(args);
        super::process::ManagedProcess::spawn(
            log_name.to_owned(),
            &mut command,
            self.logs_dir().join(format!("{log_name}.log")),
        )
    }

    pub fn keep(&self) -> bool {
        matches!(
            std::env::var("FLEET_E2E_KEEP").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        )
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let Some(dir) = self.dir.take() else {
            return;
        };
        let keep = self.keep() || !self.completed() || std::thread::panicking();
        if keep {
            eprintln!("fleet E2E artifacts kept in {}", dir.path().display());
            std::mem::forget(dir);
        }
    }
}

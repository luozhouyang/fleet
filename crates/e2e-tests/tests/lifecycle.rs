//! Fallback-supervisor lifecycle E2E: start, status, logs, restart and stop
//! without systemd or sudo.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use fleet_e2e_tests::harness::wait::wait_until;
use fleet_e2e_tests::{free_port, TestEnv};
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "end-to-end test: run with `pnpm e2e`"]
async fn supervisor_lifecycle() -> Result<()> {
    let env = TestEnv::new()?;
    let port = free_port();
    let daemon_port = free_port();
    let admin_dir = env.path().join("admin");
    let daemon_dir = env.path().join("daemon");
    std::fs::create_dir_all(&admin_dir)?;
    std::fs::create_dir_all(&daemon_dir)?;

    let admin_config = admin_dir.join("config.toml");
    let admin_db = admin_dir.join("fleet-admin.db");
    let daemon_config = daemon_dir.join("config.toml");
    let daemon_db = daemon_dir.join("fleet-daemon.db");
    let admin_state_file = admin_dir.join("run").join("supervisor.json");
    let daemon_state_file = daemon_dir.join("run").join("supervisor.json");
    let _cleanup =
        SupervisorCleanup::new(vec![admin_state_file.clone(), daemon_state_file.clone()]);

    env.fleet_ok(&[
        "admin",
        "--user",
        "setup",
        "--yes",
        "--no-install-service",
        "--no-web",
        "--config",
        path(&admin_config)?,
        "--state-dir",
        path(&admin_dir)?,
        "--database",
        path(&admin_db)?,
        "--bind",
        &format!("127.0.0.1:{port}"),
    ])
    .await
    .context("fleet admin setup")?;

    env.fleet_ok(&[
        "admin",
        "--user",
        "--config",
        path(&admin_config)?,
        "--state-dir",
        path(&admin_dir)?,
        "start",
        "--mode",
        "supervisor",
    ])
    .await
    .context("fleet admin start --mode supervisor")?;

    wait_until(
        "the admin supervisor state file",
        Duration::from_secs(20),
        || {
            let state = admin_state_file.clone();
            let logs = admin_dir.join("logs");
            async move {
                if state.is_file()
                    && logs.join("admin.log").is_file()
                    && logs.join("supervisor.log").is_file()
                {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("supervisor files are not ready yet"))
                }
            }
        },
    )
    .await?;

    let state = read_json(&admin_state_file)?;
    if state["supervisor_pid"].as_u64().is_none() || state["child_pid"].as_u64().is_none() {
        bail!("supervisor state is missing pids: {state}");
    }

    let status = env
        .fleet_ok(&[
            "--json",
            "admin",
            "--user",
            "--config",
            path(&admin_config)?,
            "--state-dir",
            path(&admin_dir)?,
            "status",
            "--mode",
            "supervisor",
        ])
        .await?;
    if !status.contains("\"mode\": \"supervisor\"") || !status.contains("\"healthy\": true") {
        bail!("admin status is not healthy: {status}");
    }

    env.fleet_ok(&[
        "admin",
        "--user",
        "--config",
        path(&admin_config)?,
        "--state-dir",
        path(&admin_dir)?,
        "logs",
        "-n",
        "20",
        "--mode",
        "supervisor",
    ])
    .await?;

    let token = admin_token(&read(&admin_config)?)?;

    env.fleet_ok(&[
        "--admin-url",
        &format!("http://127.0.0.1:{port}"),
        "--admin-token",
        &token,
        "daemon",
        "--user",
        "setup",
        "--yes",
        "--no-install-service",
        "--configure-only",
        "--store-token",
        "--config",
        path(&daemon_config)?,
        "--state-dir",
        path(&daemon_dir)?,
        "--database",
        path(&daemon_db)?,
        "--server",
        &format!("ws://127.0.0.1:{port}/ws/machine"),
        "--name",
        "lifecycle-daemon",
    ])
    .await
    .context("fleet daemon setup --configure-only")?;

    append(
        &daemon_config,
        &format!("\n[daemon]\nport = {daemon_port}\n"),
    )?;

    env.fleet_ok(&[
        "daemon",
        "--user",
        "--config",
        path(&daemon_config)?,
        "--state-dir",
        path(&daemon_dir)?,
        "start",
        "--mode",
        "supervisor",
    ])
    .await
    .context("fleet daemon start --mode supervisor")?;

    wait_until(
        "the daemon supervisor state file",
        Duration::from_secs(20),
        || {
            let state = daemon_state_file.clone();
            async move {
                if state.is_file() {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("daemon supervisor state is not ready yet"))
                }
            }
        },
    )
    .await?;

    let top = env.fleet_ok(&["--json", "status"]).await?;
    if !top.contains("\"ok\": true") || !top.contains("\"healthy\": true") {
        bail!("top-level status is not healthy: {top}");
    }

    let doctor = env.fleet_ok(&["doctor"]).await?;
    if !doctor.lines().any(|line| line.starts_with("PASS")) {
        bail!("fleet doctor did not report a PASS line:\n{doctor}");
    }

    let old_pid = read_json(&admin_state_file)?["supervisor_pid"]
        .as_u64()
        .context("admin supervisor pid")?;
    env.fleet_ok(&[
        "admin",
        "--user",
        "--config",
        path(&admin_config)?,
        "--state-dir",
        path(&admin_dir)?,
        "restart",
        "--mode",
        "supervisor",
    ])
    .await
    .context("fleet admin restart --mode supervisor")?;

    wait_until(
        "the admin supervisor to restart",
        Duration::from_secs(20),
        || {
            let state_file = admin_state_file.clone();
            async move {
                let state = read_json(&state_file)?;
                match state["supervisor_pid"].as_u64() {
                    Some(pid) if pid != old_pid => {
                        if state["restarts"].as_u64() == Some(0) {
                            Ok(())
                        } else {
                            Err(anyhow::anyhow!("restarts is {:?}", state["restarts"]))
                        }
                    }
                    Some(pid) => Err(anyhow::anyhow!("supervisor pid is still {pid}")),
                    None => Err(anyhow::anyhow!("supervisor pid is missing")),
                }
            }
        },
    )
    .await?;

    env.fleet_ok(&[
        "daemon",
        "--user",
        "--config",
        path(&daemon_config)?,
        "--state-dir",
        path(&daemon_dir)?,
        "stop",
        "--mode",
        "supervisor",
    ])
    .await?;
    wait_until(
        "the daemon supervisor to stop",
        Duration::from_secs(20),
        || {
            let state = daemon_state_file.clone();
            async move {
                if state.is_file() {
                    Err(anyhow::anyhow!("daemon supervisor state still exists"))
                } else {
                    Ok(())
                }
            }
        },
    )
    .await?;

    env.fleet_ok(&[
        "admin",
        "--user",
        "--config",
        path(&admin_config)?,
        "--state-dir",
        path(&admin_dir)?,
        "stop",
        "--mode",
        "supervisor",
    ])
    .await?;
    wait_until(
        "the admin supervisor to stop",
        Duration::from_secs(20),
        || {
            let state = admin_state_file.clone();
            async move {
                if state.is_file() {
                    Err(anyhow::anyhow!("admin supervisor state still exists"))
                } else {
                    Ok(())
                }
            }
        },
    )
    .await?;

    let status_output = env
        .fleet()
        .args([
            "admin",
            "--user",
            "--config",
            path(&admin_config)?,
            "--state-dir",
            path(&admin_dir)?,
            "status",
            "--mode",
            "supervisor",
        ])
        .output()
        .await?;
    if status_output.status.code() != Some(1) {
        bail!(
            "stopped admin status should exit 1, got {}\nstdout:\n{}\nstderr:\n{}",
            status_output.status,
            String::from_utf8_lossy(&status_output.stdout),
            String::from_utf8_lossy(&status_output.stderr),
        );
    }

    wait_until(
        "the admin health endpoint to stop responding",
        Duration::from_secs(10),
        || {
            let url = format!("http://127.0.0.1:{port}/api/v1/health");
            async move {
                match reqwest::Client::new().get(&url).send().await {
                    Ok(response) if response.status().is_success() => {
                        Err(anyhow::anyhow!("admin is still healthy"))
                    }
                    _ => Ok(()),
                }
            }
        },
    )
    .await?;

    env.finish();
    Ok(())
}

fn path(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_str(&read(path)?).with_context(|| format!("parse {}", path.display()))
}

fn append(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("append to {}", path.display()))
}

fn admin_token(config: &str) -> Result<String> {
    config
        .lines()
        .find_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("admin_token")?.trim_start();
            let rest = rest.strip_prefix('=')?.trim();
            rest.strip_prefix('"')?.strip_suffix('"').map(str::to_owned)
        })
        .ok_or_else(|| anyhow::anyhow!("admin config does not contain an admin_token"))
}

/// Kill any supervisor left behind if the test fails mid-flight.
struct SupervisorCleanup {
    state_files: Vec<PathBuf>,
}

impl SupervisorCleanup {
    fn new(state_files: Vec<PathBuf>) -> Self {
        Self { state_files }
    }
}

impl Drop for SupervisorCleanup {
    fn drop(&mut self) {
        for state_file in &self.state_files {
            let Ok(text) = std::fs::read_to_string(state_file) else {
                continue;
            };
            let Ok(state) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            for key in ["child_pid", "supervisor_pid"] {
                if let Some(pid) = state[key].as_u64() {
                    let _ = Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                }
            }
        }
    }
}

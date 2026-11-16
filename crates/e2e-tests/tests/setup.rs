//! Setup E2E: role setup, validation, identity and service planning without
//! sudo, systemd or the developer's real `~/.fleet`.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use fleet_e2e_tests::harness::wait::wait_until;
use fleet_e2e_tests::{Api, TestEnv};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "end-to-end test: run with `pnpm e2e`"]
async fn role_setup_is_hermetic() -> Result<()> {
    let env = TestEnv::new()?;
    let port = fleet_e2e_tests::harness::free_port();
    let admin_dir = env.path().join("admin");
    let daemon_dir = env.path().join("daemon");
    std::fs::create_dir_all(&admin_dir)?;
    std::fs::create_dir_all(&daemon_dir)?;

    let admin_config = admin_dir.join("config.toml");
    let admin_db = admin_dir.join("fleet-admin.db");
    let daemon_config = daemon_dir.join("config.toml");
    let daemon_db = daemon_dir.join("fleet-daemon.db");

    env.fleet_ok(&[
        "admin",
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

    let admin_text = read(&admin_config)?;
    if !admin_text.contains("admin_token = \"fla_") {
        bail!("admin config does not contain a generated admin token:\n{admin_text}");
    }
    if !admin_text.contains(&format!("database = \"{}\"", admin_db.display())) {
        bail!("admin config does not contain the requested database:\n{admin_text}");
    }
    if !env.cli_config.is_file() {
        bail!("setup did not write {}", env.cli_config.display());
    }

    let token = admin_token(&admin_text)?;
    let mut admin = env.spawn(
        &env.bins.admin,
        &["--config", path(&admin_config)?, "serve"],
        "setup-admin",
    )?;
    let api = Api::new(format!("http://127.0.0.1:{port}"), &token);
    wait_until(
        "the configured admin health check",
        Duration::from_secs(20),
        || {
            let api = api.clone();
            async move {
                let health = api.get_value("/api/v1/health").await?;
                if health["ok"] == true {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("unexpected health payload: {health}"))
                }
            }
        },
    )
    .await?;

    env.fleet_ok(&[
        "--admin-url",
        &format!("http://127.0.0.1:{port}"),
        "--admin-token",
        &token,
        "daemon",
        "setup",
        "--yes",
        "--no-install-service",
        "--configure-only",
        "--config",
        path(&daemon_config)?,
        "--state-dir",
        path(&daemon_dir)?,
        "--database",
        path(&daemon_db)?,
        "--server",
        &format!("ws://127.0.0.1:{port}/ws/machine"),
        "--name",
        "setup-test",
    ])
    .await
    .context("fleet daemon setup --configure-only")?;

    let daemon_text = read(&daemon_config)?;
    if !daemon_text.contains(&format!("server_url = \"ws://127.0.0.1:{port}")) {
        bail!("daemon config does not point at the admin:\n{daemon_text}");
    }
    if !daemon_text.contains(&format!("database = \"{}\"", daemon_db.display())) {
        bail!("daemon config does not contain the requested database:\n{daemon_text}");
    }
    if daemon_text.contains("pairing_code") {
        bail!("daemon config still contains a pairing code:\n{daemon_text}");
    }
    if daemon_text
        .lines()
        .any(|line| line.trim_start().starts_with("token") && line.contains('='))
    {
        bail!("daemon config still contains a machine token:\n{daemon_text}");
    }

    env.fleet_ok(&[
        "admin",
        "config",
        "validate",
        "--config",
        path(&admin_config)?,
    ])
    .await?;
    env.fleet_ok(&[
        "daemon",
        "config",
        "validate",
        "--config",
        path(&daemon_config)?,
    ])
    .await?;

    let identity = env
        .fleet_ok(&[
            "daemon",
            "identity",
            "show",
            "--config",
            path(&daemon_config)?,
        ])
        .await?;
    if !identity.contains("\"paired\": false") {
        bail!("daemon identity should be unpaired:\n{identity}");
    }

    let admin_service = env
        .fleet_ok(&[
            "admin",
            "install-service",
            "--dry-run",
            "--config",
            path(&admin_config)?,
        ])
        .await?;
    let daemon_service = env
        .fleet_ok(&[
            "daemon",
            "install-service",
            "--dry-run",
            "--config",
            path(&daemon_config)?,
        ])
        .await?;
    if !admin_service.contains("admin run --config") {
        bail!("admin service unit does not use `admin run --config`:\n{admin_service}");
    }
    if !daemon_service.contains("daemon run --config") {
        bail!("daemon service unit does not use `daemon run --config`:\n{daemon_service}");
    }
    for (label, unit) in [("admin", &admin_service), ("daemon", &daemon_service)] {
        if has_placeholder(unit) {
            bail!("{label} service unit still contains a placeholder:\n{unit}");
        }
    }

    admin.shutdown().await?;
    env.finish();
    Ok(())
}

/// Detect `@SOME_PLACEHOLDER@` style leftovers in rendered unit files.
fn has_placeholder(text: &str) -> bool {
    text.split('@').enumerate().any(|(index, part)| {
        index % 2 == 1
            && !part.is_empty()
            && part.chars().all(|character| {
                character.is_ascii_uppercase() || character == '_' || character.is_ascii_digit()
            })
    })
}

fn path(path: &std::path::Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn read(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
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

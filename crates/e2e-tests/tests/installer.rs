//! Installer dry-run checks: systemd units reflect the requested prefix and
//! service users without root, systemd, or a built binary.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

#[test]
fn installer_dry_run_renders_units() -> Result<()> {
    let root = fleet_e2e_tests::harness::bin::workspace_root();
    let install = root.join("scripts/install/install.sh");
    let temp = tempfile::Builder::new()
        .prefix("fleet-installer-")
        .tempdir()?;

    let explicit = dry_run(
        &install,
        &[
            "--prefix",
            "/opt/fleet",
            "--admin-user",
            "adminx",
            "--daemon-user",
            "daemonx",
        ],
    )?;
    for expected in [
        "User=adminx",
        "Group=adminx",
        "User=daemonx",
        "Group=daemonx",
        "ExecStart=/opt/fleet/bin/fleet admin run --config /etc/fleet/admin.toml",
        "ExecStart=/opt/fleet/bin/fleet daemon run --config /etc/fleet/daemon.toml",
        "EnvironmentFile=-/etc/fleet/admin.env",
        "EnvironmentFile=-/etc/fleet/daemon.env",
        "ReadWritePaths=/var/lib/fleet/admin",
        "StartLimitIntervalSec=60",
        "StartLimitBurst=10",
    ] {
        assert_contains(&explicit, expected)?;
    }
    assert_absent(&explicit, "/usr/local")?;
    assert_no_placeholder(&explicit)?;
    assert_absent(&explicit, "/var/lib/fleet-admin")?;
    assert_absent(&explicit, "/var/lib/fleet-daemon")?;

    let defaults = dry_run(&install, &[])?;
    assert_contains(
        &defaults,
        "ExecStart=/usr/local/bin/fleet admin run --config /etc/fleet/admin.toml",
    )?;
    assert_contains(
        &defaults,
        "ExecStart=/usr/local/bin/fleet daemon run --config /etc/fleet/daemon.toml",
    )?;
    if count(&defaults, "User=fleet-admin") != 1 {
        bail!("the admin service should default to User=fleet-admin");
    }
    if count(&defaults, "User=fleet-daemon") != 1 {
        bail!("the daemon service should default to User=fleet-daemon");
    }
    assert_no_placeholder(&defaults)?;

    let shared = dry_run(&install, &["--user", "shared"])?;
    if count(&shared, "User=shared") != 2 {
        bail!("--user should set both service users");
    }

    let stage_path = temp.path().join("stage");
    let rooted = dry_run(
        &install,
        &[
            "--root",
            stage_path.to_str().context("stage path is UTF-8")?,
            "--prefix",
            "/opt/fleet",
            "--admin-user",
            "adminx",
            "--daemon-user",
            "daemonx",
        ],
    )?;
    let stage = stage_path.display();
    assert_contains(
        &rooted,
        &format!(
            "ExecStart={stage}/opt/fleet/bin/fleet admin run --config {stage}/etc/fleet/admin.toml"
        ),
    )?;
    assert_contains(
        &rooted,
        &format!("ExecStart={stage}/opt/fleet/bin/fleet daemon run --config {stage}/etc/fleet/daemon.toml"),
    )?;
    assert_contains(
        &rooted,
        &format!("EnvironmentFile=-{stage}/etc/fleet/admin.env"),
    )?;
    assert_contains(
        &rooted,
        &format!("ReadWritePaths={stage}/var/lib/fleet/admin"),
    )?;
    if stage_path.exists() {
        bail!("--dry-run must not create the staged root");
    }

    Ok(())
}

fn dry_run(install: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("bash")
        .arg(install)
        .arg("--dry-run")
        .args(args)
        .env_remove("SUDO_USER")
        .output()
        .with_context(|| format!("run {}", install.display()))?;
    if !output.status.success() {
        bail!(
            "installer dry-run failed ({})\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn assert_contains(text: &str, needle: &str) -> Result<()> {
    if !text.contains(needle) {
        bail!("expected {needle:?} in installer output:\n{text}");
    }
    Ok(())
}

fn assert_absent(text: &str, needle: &str) -> Result<()> {
    if text.contains(needle) {
        bail!("did not expect {needle:?} in installer output:\n{text}");
    }
    Ok(())
}

fn count(text: &str, needle: &str) -> usize {
    text.lines().filter(|line| line.starts_with(needle)).count()
}

fn assert_no_placeholder(text: &str) -> Result<()> {
    let placeholder = text.split('@').enumerate().any(|(index, part)| {
        index % 2 == 1
            && !part.is_empty()
            && part.chars().all(|character| {
                character.is_ascii_uppercase() || character == '_' || character.is_ascii_digit()
            })
    });
    if placeholder {
        bail!("installer output still contains a placeholder:\n{text}");
    }
    Ok(())
}

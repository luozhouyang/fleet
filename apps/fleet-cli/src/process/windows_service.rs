//! Native Windows Service Control Manager backend.
//!
//! The admin and daemon binaries implement the service control protocol when
//! started with `--windows-service`. This module registers, starts, stops, and
//! removes those services with `sc.exe`.
//!
//! The module is compiled on every host so its command construction can be
//! tested without Windows; only the Windows build selects it at runtime.
#![allow(dead_code)]

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::config_paths::RolePaths;

use super::service::InstallOutcome;

const STOP_TIMEOUT: Duration = Duration::from_secs(20);

pub fn service_available() -> bool {
    cfg!(windows)
}

pub fn service_name(paths: &RolePaths) -> &'static str {
    paths.role.binary_name()
}

pub fn unit_path(paths: &RolePaths) -> PathBuf {
    PathBuf::from(service_name(paths))
}

pub fn log_hint(paths: &RolePaths) -> String {
    paths
        .state_dir
        .join("logs")
        .join(format!("{}.log", paths.role.as_str()))
        .display()
        .to_string()
}

pub fn unit_installed(paths: &RolePaths) -> bool {
    query(paths)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn install(
    paths: &RolePaths,
    start: bool,
    dry_run: bool,
    _yes: bool,
) -> Result<InstallOutcome> {
    let server = paths.require_binary()?;
    let server = super::service::native_path(server);
    let config = super::service::native_path(&paths.config);
    let name = service_name(paths);
    let bin_path = format!(
        "\"{}\" --config \"{}\" serve --windows-service",
        server.display(),
        config.display()
    );
    let display_name = match paths.role {
        crate::config_paths::Role::Admin => "Fleet Admin",
        crate::config_paths::Role::Daemon => "Fleet Daemon",
    };
    let description = match paths.role {
        crate::config_paths::Role::Admin => "Fleet admin control plane",
        crate::config_paths::Role::Daemon => "Fleet machine daemon",
    };

    let service_user = std::env::var("FLEET_SERVICE_USER")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let service_password = std::env::var("FLEET_SERVICE_PASSWORD").ok();
    if service_user.is_some() && service_password.is_none() {
        bail!("FLEET_SERVICE_PASSWORD is required when FLEET_SERVICE_USER is set");
    }

    let installed = unit_installed(paths);
    let mut commands: Vec<Vec<String>> = Vec::new();
    if installed {
        let mut command = vec![
            "config".into(),
            name.into(),
            "binPath=".into(),
            bin_path.clone(),
            "start=".into(),
            "auto".into(),
        ];
        push_service_account(
            &mut command,
            service_user.as_deref(),
            service_password.as_deref(),
        );
        commands.push(command);
    } else {
        let mut command = vec![
            "create".into(),
            name.into(),
            "binPath=".into(),
            bin_path.clone(),
            "start=".into(),
            "auto".into(),
            "DisplayName=".into(),
            display_name.into(),
        ];
        push_service_account(
            &mut command,
            service_user.as_deref(),
            service_password.as_deref(),
        );
        commands.push(command);
    }
    commands.push(vec!["description".into(), name.into(), description.into()]);
    commands.push(vec![
        "failure".into(),
        name.into(),
        "reset=".into(),
        "86400".into(),
        "actions=".into(),
        "restart/5000/restart/10000/restart/30000".into(),
    ]);
    if start {
        commands.push(vec!["start".into(), name.into()]);
    }

    if dry_run {
        for command in &commands {
            println!("{}", display_sc(command));
        }
        return Ok(InstallOutcome {
            unit_path: unit_path(paths),
            env_path: None,
            skipped: false,
        });
    }

    for command in &commands {
        let output = run_sc(command)?;
        if output.status.success() {
            continue;
        }
        print_plan(&commands);
        let detail = output_text(&output);
        if is_access_denied(&detail) {
            eprintln!("Windows service changes require an elevated shell; nothing was installed");
            return Ok(InstallOutcome {
                unit_path: unit_path(paths),
                env_path: None,
                skipped: true,
            });
        }
        bail!(
            "sc.exe {} failed: {}",
            command.first().map(String::as_str).unwrap_or(""),
            detail
        );
    }

    println!(
        "installed Windows service {name}; logs: {}",
        log_hint(paths)
    );
    Ok(InstallOutcome {
        unit_path: unit_path(paths),
        env_path: None,
        skipped: false,
    })
}

pub fn uninstall(paths: &RolePaths, dry_run: bool, _yes: bool) -> Result<()> {
    let name = service_name(paths);
    if dry_run {
        println!("sc.exe stop {name}");
        println!("sc.exe delete {name}");
        return Ok(());
    }

    let stop = run_sc(&["stop".into(), name.into()])?;
    if !stop.status.success() && !is_not_installed(&output_text(&stop)) {
        // Stopping a service that is already stopped returns a non-zero code on
        // some Windows versions; deletion is still the desired end state.
        let detail = output_text(&stop);
        if is_access_denied(&detail) {
            eprintln!("run `sc.exe stop {name}` and `sc.exe delete {name}` from an elevated shell");
            return Ok(());
        }
    }

    let delete = run_sc(&["delete".into(), name.into()])?;
    if delete.status.success() || is_not_installed(&output_text(&delete)) {
        println!("removed Windows service {name}");
        return Ok(());
    }
    let detail = output_text(&delete);
    if is_access_denied(&detail) {
        eprintln!("run `sc.exe delete {name}` from an elevated shell");
        return Ok(());
    }
    bail!("sc.exe delete {name} failed: {detail}");
}

pub fn start(paths: &RolePaths) -> Result<()> {
    let name = service_name(paths);
    let output = run_sc(&["start".into(), name.into()])?;
    if output.status.success() || is_already_running(&output_text(&output)) {
        return Ok(());
    }
    bail!("sc.exe start {name} failed: {}", output_text(&output));
}

pub fn stop(paths: &RolePaths) -> Result<()> {
    let name = service_name(paths);
    let output = run_sc(&["stop".into(), name.into()])?;
    let detail = output_text(&output);
    if !output.status.success() && !is_not_running(&detail) && !is_not_installed(&detail) {
        bail!("sc.exe stop {name} failed: {detail}");
    }
    wait_for_stopped(paths)?;
    println!("{} stopped", paths.role.as_str());
    Ok(())
}

pub fn restart(paths: &RolePaths) -> Result<()> {
    stop(paths)?;
    start(paths)
}

pub fn status(paths: &RolePaths) -> super::service::ServiceStatus {
    let Ok(output) = query(paths) else {
        return super::service::ServiceStatus {
            backend: "windows-service",
            active: false,
            state: "unknown".to_string(),
            detail: "sc.exe query failed".to_string(),
        };
    };
    if !output.status.success() {
        return super::service::ServiceStatus {
            backend: "windows-service",
            active: false,
            state: "not installed".to_string(),
            detail: output_text(&output),
        };
    }
    let text = output_text(&output);
    let (state, active) = parse_state(&text);
    super::service::ServiceStatus {
        backend: "windows-service",
        active,
        state,
        detail: text.trim().to_string(),
    }
}

pub fn logs(paths: &RolePaths, lines: usize, follow: bool) -> Result<()> {
    let path = paths
        .state_dir
        .join("logs")
        .join(format!("{}.log", paths.role.as_str()));
    if !path.is_file() {
        bail!(
            "no Windows service log at {}; start the service first",
            path.display()
        );
    }
    if follow {
        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Get-Content -LiteralPath '{}' -Tail {} -Wait",
                    path.display().to_string().replace('\'', "''"),
                    lines
                ),
            ])
            .status()
            .with_context(|| "failed to start powershell.exe")?;
        if !status.success() {
            bail!("powershell.exe exited with {status}");
        }
        return Ok(());
    }

    let mut file =
        File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
    let len = file.metadata()?.len();
    let mut buf = Vec::new();
    let read = len.min(1024 * 1024);
    file.seek(SeekFrom::Start(len.saturating_sub(read)))?;
    file.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    for line in &all[start..] {
        println!("{line}");
    }
    Ok(())
}

fn query(paths: &RolePaths) -> Result<Output> {
    run_sc(&["query".into(), service_name(paths).into()])
}

fn wait_for_stopped(paths: &RolePaths) -> Result<()> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        let output = query(paths)?;
        let text = output_text(&output);
        let (state, active) = parse_state(&text);
        if !output.status.success() || (!active && state != "stop_pending") {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "Windows service {} did not stop within {STOP_TIMEOUT:?}",
                service_name(paths)
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn run_sc(args: &[String]) -> Result<Output> {
    Command::new("sc.exe")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| "failed to run sc.exe")
}

fn output_text(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn is_access_denied(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("access is denied")
        || lower.contains("拒绝访问")
        || lower.contains("error 5")
        || lower.contains("失败 5")
}

fn is_not_installed(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("1060") || lower.contains("does not exist") || lower.contains("不存在")
}

fn is_already_running(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("1056") || lower.contains("already running") || lower.contains("已经运行")
}

fn is_not_running(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("1062") || lower.contains("not running") || lower.contains("没有运行")
}

/// Parse the numeric service state. sc.exe localizes labels, but the state
/// code (1 = stopped, 4 = running) is stable.
fn parse_state(text: &str) -> (String, bool) {
    for line in text.lines() {
        for token in line.split_whitespace() {
            if let Ok(code) = token.parse::<u32>() {
                let state = match code {
                    1 => "stopped",
                    2 => "start_pending",
                    3 => "stop_pending",
                    4 => "running",
                    5 => "continue_pending",
                    6 => "pause_pending",
                    7 => "paused",
                    _ => continue,
                };
                return (state.to_string(), code == 4);
            }
        }
    }
    ("unknown".to_string(), false)
}

fn push_service_account(command: &mut Vec<String>, user: Option<&str>, password: Option<&str>) {
    if let Some(user) = user {
        command.push("obj=".into());
        command.push(user.to_string());
        if let Some(password) = password {
            command.push("password=".into());
            command.push(password.to_string());
        }
    }
}

fn print_plan(commands: &[Vec<String>]) {
    eprintln!("run these commands from an elevated shell:");
    for command in commands {
        eprintln!("  {}", display_sc(command));
    }
}

fn display_sc(command: &[String]) -> String {
    let mut line = String::from("sc.exe");
    let mut mask_next = false;
    for arg in command {
        line.push(' ');
        if mask_next {
            line.push_str("***");
            mask_next = false;
            continue;
        }
        if arg.contains(' ') {
            line.push('"');
            line.push_str(&arg.replace('"', "\\\""));
            line.push('"');
        } else {
            line.push_str(arg);
        }
        if arg == "password=" {
            mask_next = true;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_running_state_with_localized_labels() {
        let text = "SERVICE_NAME: fleet-admin\n        TYPE               : 10  WIN32_OWN_PROCESS\n        STATE              : 4  RUNNING\n        WIN32_EXIT_CODE    : 0\n";
        let (state, active) = parse_state(text);
        assert_eq!(state, "running");
        assert!(active);
    }

    #[test]
    fn parses_stopped_state() {
        let text = "        STATE              : 1  STOPPED\n        WIN32_EXIT_CODE    : 0\n";
        let (state, active) = parse_state(text);
        assert_eq!(state, "stopped");
        assert!(!active);
    }

    #[test]
    fn command_display_quotes_spaced_arguments() {
        let line = display_sc(&[
            "create".into(),
            "fleet-admin".into(),
            "binPath=".into(),
            "\"C:\\Program Files\\Fleet\\fleet-admin.exe\" serve".into(),
        ]);
        assert!(line.starts_with("sc.exe create fleet-admin binPath="));
        assert!(line.contains("\\\"C:\\Program Files\\Fleet\\fleet-admin.exe\\\" serve"));
    }
}

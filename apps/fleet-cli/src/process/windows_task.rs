//! Windows Task Scheduler backend for Fleet services.
//!
//! The server profile starts at boot and must run as the invoking user: the
//! daemon needs that user's profile, repositories, and agent CLIs, and the
//! admin must read the user's configuration. Tasks therefore use a boot
//! trigger with a stored credential instead of a Session 0 service.
//!
//! The module is compiled on every host so its task XML can be tested without
//! Windows; only the Windows build selects it at runtime.
#![allow(dead_code)]

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Context, Result};

use crate::config_paths::{Role, RolePaths};

use super::service::InstallOutcome;

pub fn task_available() -> bool {
    cfg!(windows)
}

pub fn task_name(paths: &RolePaths) -> &'static str {
    match paths.role {
        Role::Admin => "fleet-admin",
        Role::Daemon => "fleet-daemon",
    }
}

pub fn unit_path(paths: &RolePaths) -> PathBuf {
    PathBuf::from(task_name(paths))
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
    let name = task_name(paths);
    let xml = render_task_xml(paths)?;
    let temp = std::env::temp_dir().join(format!("{name}-{}.xml", std::process::id()));
    write_task_xml(&temp, &xml).with_context(|| format!("failed to write {}", temp.display()))?;

    let create = vec![
        "/Create".to_string(),
        "/TN".to_string(),
        task_name(paths).to_string(),
        "/XML".to_string(),
        temp.display().to_string(),
        "/F".to_string(),
    ];
    let run = vec![
        "/Run".to_string(),
        "/TN".to_string(),
        task_name(paths).to_string(),
    ];

    if dry_run {
        println!("<task xml>");
        println!("{xml}");
        println!("{}", display_schtasks(&create));
        if start {
            println!("{}", display_schtasks(&run));
        }
        let _ = fs::remove_file(&temp);
        return Ok(InstallOutcome {
            unit_path: unit_path(paths),
            env_path: None,
            skipped: false,
        });
    }

    let output = run_schtasks(&create)?;
    let _ = fs::remove_file(&temp);
    if !output.status.success() {
        let detail = output_text(&output);
        if is_access_denied(&detail) {
            eprintln!("creating a Task Scheduler task requires the right privileges");
            eprintln!("run these commands from an elevated shell:");
            eprintln!("{}", display_schtasks(&create));
            if start {
                eprintln!("{}", display_schtasks(&run));
            }
            return Ok(InstallOutcome {
                unit_path: unit_path(paths),
                env_path: None,
                skipped: true,
            });
        }
        bail!("schtasks /Create failed: {detail}");
    }

    if start {
        let output = run_schtasks(&run)?;
        if !output.status.success() && !is_already_running(&output_text(&output)) {
            bail!("schtasks /Run failed: {}", output_text(&output));
        }
    }

    println!(
        "installed Task Scheduler task {name}; logs: {}",
        log_hint(paths)
    );
    Ok(InstallOutcome {
        unit_path: unit_path(paths),
        env_path: None,
        skipped: false,
    })
}

pub fn uninstall(paths: &RolePaths, dry_run: bool, _yes: bool) -> Result<()> {
    let name = task_name(paths);
    let delete = vec![
        "/Delete".to_string(),
        "/TN".to_string(),
        name.to_string(),
        "/F".to_string(),
    ];
    if dry_run {
        println!("schtasks /End /TN {name}");
        println!("{}", display_schtasks(&delete));
        return Ok(());
    }
    let _ = run_schtasks(&["/End".to_string(), "/TN".to_string(), name.to_string()]);
    let output = run_schtasks(&delete)?;
    if output.status.success() || is_not_installed(&output_text(&output)) {
        println!("removed Task Scheduler task {name}");
        return Ok(());
    }
    bail!("schtasks /Delete failed: {}", output_text(&output));
}

pub fn start(paths: &RolePaths) -> Result<()> {
    let output = run_schtasks(&[
        "/Run".to_string(),
        "/TN".to_string(),
        task_name(paths).to_string(),
    ])?;
    if output.status.success() || is_already_running(&output_text(&output)) {
        return Ok(());
    }
    bail!("schtasks /Run failed: {}", output_text(&output));
}

pub fn stop(paths: &RolePaths) -> Result<()> {
    let output = run_schtasks(&[
        "/End".to_string(),
        "/TN".to_string(),
        task_name(paths).to_string(),
    ])?;
    if !output.status.success() && !is_not_running(&output_text(&output)) {
        bail!("schtasks /End failed: {}", output_text(&output));
    }
    println!("{} stopped", paths.role.as_str());
    Ok(())
}

pub fn restart(paths: &RolePaths) -> Result<()> {
    stop(paths)?;
    start(paths)
}

pub fn status(paths: &RolePaths) -> super::service::ServiceStatus {
    // `Get-ScheduledTask` returns a language-independent state enum. Fall back
    // to parsing `schtasks` output when PowerShell or the cmdlet is missing.
    if let Some(raw) = powershell_state(paths) {
        let state = normalize_state(&raw);
        return super::service::ServiceStatus {
            backend: "windows-task",
            active: state == "running",
            state,
            detail: format!("Get-ScheduledTask State={raw}"),
        };
    }

    let output = query(paths);
    let Ok(output) = output else {
        return super::service::ServiceStatus {
            backend: "windows-task",
            active: false,
            state: "unknown".to_string(),
            detail: "schtasks query failed".to_string(),
        };
    };
    if !output.status.success() {
        return super::service::ServiceStatus {
            backend: "windows-task",
            active: false,
            state: "not installed".to_string(),
            detail: output_text(&output),
        };
    }
    let detail = output_text(&output);
    let state = parse_state(&detail);
    let active = state == "running";
    super::service::ServiceStatus {
        backend: "windows-task",
        active,
        state,
        detail,
    }
}

pub fn logs(paths: &RolePaths, lines: usize, follow: bool) -> Result<()> {
    let path = PathBuf::from(log_hint(paths));
    if !path.is_file() {
        bail!(
            "no Task Scheduler {} log at {}; start the task first",
            paths.role.as_str(),
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
        fs::File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
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

/// Render the task definition. Kept separate from installation so the XML can
/// be tested on non-Windows hosts.
pub fn render_task_xml(paths: &RolePaths) -> Result<String> {
    let server = paths.require_binary()?;
    let server = super::service::native_path(server);
    let config = super::service::native_path(&paths.config);
    let arguments = format!("--config \"{}\" serve --windows-task", config.display());
    let (description, run_level) = match paths.role {
        Role::Admin => ("Fleet admin control plane", "LeastPrivilege"),
        Role::Daemon => ("Fleet machine daemon", "LeastPrivilege"),
    };
    let trigger = "<BootTrigger><Enabled>true</Enabled></BootTrigger>".to_string();
    let principal = format!(
        "<UserId>{}</UserId><LogonType>Password</LogonType>",
        escape(&paths.service_user)
    );

    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{}</Description>
  </RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals>
    <Principal id="Author">
      {principal}
      <RunLevel>{run_level}</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>{}</Arguments>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        escape(description),
        escape(&server.display().to_string()),
        escape(&arguments),
        escape(&paths.state_dir.display().to_string()),
    ))
}

fn query(paths: &RolePaths) -> Result<Output> {
    Command::new("schtasks.exe")
        .args(["/Query", "/TN", task_name(paths), "/FO", "LIST"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| "failed to run schtasks.exe")
}

fn run_schtasks(args: &[String]) -> Result<Output> {
    Command::new("schtasks.exe")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| "failed to run schtasks.exe")
}

fn output_text(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn parse_state(text: &str) -> String {
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            if key.eq_ignore_ascii_case("Status")
                || key.contains("状态")
                || key.eq_ignore_ascii_case("Estado")
            {
                return normalize_state(value);
            }
        }
    }
    "unknown".to_string()
}

fn normalize_state(value: &str) -> String {
    let value = value.trim().to_lowercase();
    if value.contains("running") || value.contains("正在运行") || value.contains("ejecutando") {
        return "running".to_string();
    }
    if value.contains("queued")
        || value.contains("ready")
        || value.contains("就绪")
        || value.contains("listo")
    {
        return "ready".to_string();
    }
    if value.contains("disabled") || value.contains("已禁用") || value.contains("deshabilitado")
    {
        return "disabled".to_string();
    }
    value
}

fn powershell_state(paths: &RolePaths) -> Option<String> {
    let name = task_name(paths);
    let script = format!("$t = Get-ScheduledTask -TaskName '{name}' -ErrorAction Stop; $t.State");
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!state.is_empty()).then_some(state)
}

fn is_access_denied(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("access is denied") || lower.contains("拒绝访问") || lower.contains("error 5")
}

fn is_not_installed(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("cannot find") || lower.contains("does not exist") || lower.contains("找不到")
}

fn is_already_running(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("already running") || lower.contains("已经在运行")
}

fn is_not_running(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("not running") || lower.contains("没有运行")
}

fn write_task_xml(path: &Path, xml: &str) -> std::io::Result<()> {
    // schtasks /XML expects the Unicode form, so write UTF-16LE with a BOM.
    let mut bytes = Vec::with_capacity(xml.len() * 2 + 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(path, bytes)
}

fn display_schtasks(args: &[String]) -> String {
    let mut line = String::from("schtasks.exe");
    for arg in args {
        line.push(' ');
        if arg.contains(' ') {
            line.push('"');
            line.push_str(&arg.replace('"', "\\\""));
            line.push('"');
        } else {
            line.push_str(arg);
        }
    }
    line
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_paths::RolePathOverrides;

    fn paths(home: &Path) -> RolePaths {
        paths_for(Role::Daemon, home)
    }

    fn paths_for(role: Role, home: &Path) -> RolePaths {
        let overrides = RolePathOverrides {
            bin: Some(std::env::current_exe().unwrap()),
            ..RolePathOverrides::default()
        };
        RolePaths::resolve_with(role, &overrides, "dev", "dev", home).unwrap()
    }

    #[test]
    fn task_xml_uses_a_boot_trigger_with_the_invoking_user() {
        let home = Path::new("/Users/dev");
        let xml = render_task_xml(&paths(home)).unwrap();
        assert!(xml.contains("<BootTrigger>"));
        assert!(xml.contains("<UserId>dev</UserId>"));
        assert!(xml.contains("<LogonType>Password</LogonType>"));
        assert!(xml.contains("--windows-task"));
        assert!(xml.contains("<RestartOnFailure>"));
    }

    #[test]
    fn admin_task_xml_uses_the_admin_binary_and_task_name() {
        let home = Path::new("/Users/dev");
        let paths = paths_for(Role::Admin, home);
        assert_eq!(task_name(&paths), "fleet-admin");
        let xml = render_task_xml(&paths).unwrap();
        assert!(xml.contains("Fleet admin control plane"));
        assert!(xml.contains("serve --windows-task"));
    }

    #[test]
    fn parses_task_status() {
        assert_eq!(parse_state("Status: Running"), "running");
        assert_eq!(parse_state("Status: Ready"), "ready");
        assert_eq!(parse_state("Status: Disabled"), "disabled");
        assert_eq!(parse_state("状态: 正在运行"), "running");
        assert_eq!(parse_state("状态: 就绪"), "ready");
        assert_eq!(parse_state("状态: 已禁用"), "disabled");
    }

    #[test]
    fn normalizes_powershell_states() {
        assert_eq!(normalize_state("Running"), "running");
        assert_eq!(normalize_state("Ready"), "ready");
        assert_eq!(normalize_state("Disabled"), "disabled");
        assert_eq!(normalize_state("Unknown"), "unknown");
    }
}

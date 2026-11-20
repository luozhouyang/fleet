//! Renders and installs the systemd units for the admin and daemon roles.
//!
//! The unit templates are embedded from `packaging/systemd` so the installer
//! and the CLI always emit the same files. Tests never execute `sudo`,
//! `systemctl`, or `useradd`; they inspect [`ServicePlan`] instead.

#[cfg(test)]
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config::{create_private_dir, home_dir, write_private};
use crate::config_paths::{is_root, Role, RolePaths};

const ADMIN_TEMPLATE: &str = include_str!("../../../../packaging/systemd/fleet-admin.service.in");
const DAEMON_TEMPLATE: &str = include_str!("../../../../packaging/systemd/fleet-daemon.service.in");

/// Render the unit for `paths`. User scope drops the privileged environment
/// file and the `ProtectHome` restriction.
#[allow(dead_code)] // public entry point for future diagnostics
pub fn render_unit(paths: &RolePaths) -> Result<String> {
    render_unit_with_home(paths, &home_dir())
}

fn render_unit_with_home(paths: &RolePaths, user_home: &Path) -> Result<String> {
    let template = match paths.role {
        Role::Admin => ADMIN_TEMPLATE,
        Role::Daemon => DAEMON_TEMPLATE,
    };
    let home = user_home;

    // The service runs at boot as the invoking user and reads that user's
    // configuration directly; there is no privileged environment file and the
    // unit must be able to access the user's home.
    let mut unit = template.to_string();
    unit = unit.replace("EnvironmentFile=-@ENV_FILE@\n", "");
    unit = unit.replace("ProtectHome=true\n", "");
    unit = unit
        .replace("@ADMIN_USER@", &paths.service_user)
        .replace("@ADMIN_GROUP@", &paths.service_group)
        .replace("@DAEMON_USER@", &paths.service_user)
        .replace("@DAEMON_GROUP@", &paths.service_group)
        .replace("@ADMIN_STATE_DIR@", &paths.state_dir.display().to_string())
        .replace("@DAEMON_STATE_DIR@", &paths.state_dir.display().to_string())
        .replace("@ADMIN_HOME@", &home.display().to_string())
        .replace("@DAEMON_HOME@", &home.display().to_string())
        .replace("@FLEET_BIN@", &paths.fleet_binary.display().to_string())
        .replace("@CONFIG_FILE@", &paths.config.display().to_string())
        .replace("@ENV_FILE@", "");
    Ok(unit)
}

/// True when this role's unit file exists.
pub fn unit_installed(paths: &RolePaths) -> bool {
    unit_path(paths).is_file()
}

/// Config path used by the installed unit, parsed from `ExecStart`.
pub fn unit_config_path(paths: &RolePaths) -> Option<PathBuf> {
    let text = std::fs::read_to_string(unit_path(paths)).ok()?;
    let marker = "--config ";
    let line = text.lines().find(|line| line.contains(marker))?;
    let value = line.split(marker).nth(1)?.trim();
    let value = value.split_whitespace().next().unwrap_or(value);
    Some(PathBuf::from(value))
}

/// `systemctl start|stop|restart <unit>`.
///
/// System units need root. The CLI first tries the unprivileged command, then
/// retries through `sudo` only when the failure is a privilege error.
pub fn start(paths: &RolePaths) -> Result<()> {
    run_action(paths, "start")
}

/// See [`start`].
pub fn stop(paths: &RolePaths) -> Result<()> {
    run_action(paths, "stop")
}

/// See [`start`].
pub fn restart(paths: &RolePaths) -> Result<()> {
    run_action(paths, "restart")
}

fn run_action(paths: &RolePaths, action: &str) -> Result<()> {
    run_action_with_sudo(Path::new("systemctl"), paths.role.unit_name(), action, true)
}

#[cfg(test)]
fn run_action_at(program: &Path, unit: &str, action: &str) -> Result<()> {
    run_action_with_sudo(program, unit, action, false)
}

fn run_action_with_sudo(program: &Path, unit: &str, action: &str, allow_sudo: bool) -> Result<()> {
    let output = action_output_at(program, unit, action)?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if allow_sudo && !is_root() && privilege_error(detail) && run_sudo_action(program, unit, action)
    {
        return Ok(());
    }
    anyhow::bail!(
        "systemctl {action} {unit} failed{}{}; run: sudo systemctl {action} {unit}",
        if detail.is_empty() { "" } else { ": " },
        detail
    );
}

fn privilege_error(detail: &str) -> bool {
    let detail = detail.to_lowercase();
    detail.contains("interactive authentication required")
        || detail.contains("access denied")
        || detail.contains("permission denied")
        || detail.contains("authentication is required")
}

fn run_sudo_action(program: &Path, unit: &str, action: &str) -> bool {
    let mut command = Command::new("sudo");
    if !io::stdin().is_terminal() {
        command.arg("-n");
    }
    command
        .arg(program)
        .arg(action)
        .arg(unit)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn action_output_at(program: &Path, unit: &str, action: &str) -> Result<std::process::Output> {
    Command::new(program)
        .arg(action)
        .arg(unit)
        .output()
        .with_context(|| format!("failed to run {}", program.display()))
}

/// Snapshot of the unit state used by `fleet ... status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdStatus {
    pub active: bool,
    pub unit_file_state: String,
    pub restart: String,
}

pub fn status(paths: &RolePaths) -> SystemdStatus {
    status_at(Path::new("systemctl"), paths.role.unit_name())
}

fn status_at(program: &Path, unit: &str) -> SystemdStatus {
    let active = Command::new(program)
        .args(["is-active", "--quiet", unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    SystemdStatus {
        active,
        unit_file_state: query_at(program, unit, "UnitFileState"),
        restart: query_at(program, unit, "Restart"),
    }
}

fn query_at(program: &Path, unit: &str, property: &str) -> String {
    Command::new(program)
        .args(["show", "-p", property, "--value", unit])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Follow the role's journal through `journalctl`.
pub fn logs(paths: &RolePaths, lines: usize, follow: bool) -> Result<()> {
    logs_at(
        Path::new("journalctl"),
        paths.role.unit_name(),
        lines,
        follow,
    )
}

fn logs_at(program: &Path, unit: &str, lines: usize, follow: bool) -> Result<()> {
    let mut command = Command::new(program);
    command.arg("-u").arg(unit).arg("-n").arg(lines.to_string());
    if follow {
        command.arg("--follow");
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run {}", program.display()))?;
    if !status.success() {
        anyhow::bail!("journalctl -u {unit} exited with {status}");
    }
    Ok(())
}

/// Absolute path of the installed unit file.
pub fn unit_path(paths: &RolePaths) -> PathBuf {
    unit_path_with_home(paths, &home_dir())
}

fn unit_path_with_home(paths: &RolePaths, _home: &Path) -> PathBuf {
    // User scope intentionally installs a system unit as the invoking user;
    // system units start at boot without `loginctl enable-linger`.
    PathBuf::from("/etc/systemd/system").join(paths.role.unit_name())
}

/// True when the host can run systemd units.
#[allow(dead_code)] // false at compile time on non-Linux hosts
pub fn systemd_available() -> bool {
    cfg!(target_os = "linux")
        && Path::new("/run/systemd/system").is_dir()
        && Command::new("systemctl")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
}

/// Whether privileged commands may be invoked through `sudo`.
///
/// Root never needs sudo. `--yes` runs non-interactively, so sudo is only
/// usable when passwordless credentials were verified. Interactive runs may
/// use sudo when a TTY is available for the password prompt.
pub fn sudo_required(is_root: bool, yes: bool, sudo_n_ok: bool, tty: bool) -> bool {
    if is_root {
        false
    } else if yes {
        sudo_n_ok
    } else {
        tty
    }
}

/// One command in a [`ServicePlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub privileged: bool,
}

impl CommandSpec {
    fn new(program: impl Into<String>, args: Vec<String>, privileged: bool) -> Self {
        Self {
            program: program.into(),
            args,
            privileged,
        }
    }

    /// Render the command for display, optionally prefixed with `sudo`.
    pub fn display(&self, is_root: bool) -> String {
        let mut line = String::new();
        if self.privileged && !is_root {
            line.push_str("sudo ");
        }
        line.push_str(&self.program);
        for arg in &self.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

/// Everything needed to install one role's unit. `env` may contain the admin
/// token, so this type deliberately does not derive `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ServicePlan {
    pub unit_name: String,
    pub unit_path: PathBuf,
    pub unit: String,
    pub env_path: Option<PathBuf>,
    pub commands: Vec<CommandSpec>,
}

/// Result of [`install`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub unit_path: PathBuf,
    pub env_path: Option<PathBuf>,
    /// True when privileged commands were printed for manual execution instead
    /// of being run.
    pub skipped: bool,
}

/// Build the installation plan without writing anything.
#[allow(dead_code)] // public entry point for future diagnostics
pub fn plan_install(
    paths: &RolePaths,
    admin_token: Option<&str>,
    start: bool,
) -> Result<ServicePlan> {
    plan_install_with_home(paths, admin_token, start, &home_dir())
}

fn plan_install_with_home(
    paths: &RolePaths,
    _admin_token: Option<&str>,
    start: bool,
    home: &Path,
) -> Result<ServicePlan> {
    let unit = render_unit_with_home(paths, home)?;
    let unit_name = paths.role.unit_name().to_string();
    let unit_path = unit_path_with_home(paths, home);
    let staging = staging_dir_with_home(home, paths.role);
    let staging_unit = staging.join(&unit_name);

    let mut commands = Vec::new();

    commands.push(CommandSpec::new(
        "install",
        vec![
            "-d".into(),
            "-m".into(),
            "0755".into(),
            "/etc/systemd/system".into(),
        ],
        true,
    ));

    commands.push(CommandSpec::new(
        "install",
        vec![
            "-m".into(),
            "0644".into(),
            staging_unit.display().to_string(),
            unit_path.display().to_string(),
        ],
        true,
    ));

    commands.push(CommandSpec::new(
        "systemctl",
        vec!["daemon-reload".into()],
        true,
    ));

    let mut enable = vec!["enable".to_string()];
    if start {
        enable.push("--now".into());
    }
    enable.push(unit_name.clone());
    commands.push(CommandSpec::new("systemctl", enable, true));

    Ok(ServicePlan {
        unit_name,
        unit_path,
        unit,
        env_path: None,
        commands,
    })
}

/// Render, stage, and install one role's unit. `dry_run` writes nothing and
/// executes nothing.
pub fn install(
    paths: &RolePaths,
    admin_token: Option<&str>,
    start: bool,
    dry_run: bool,
    yes: bool,
) -> Result<InstallOutcome> {
    install_with_home(paths, admin_token, start, dry_run, yes, &home_dir())
}

fn install_with_home(
    paths: &RolePaths,
    admin_token: Option<&str>,
    start: bool,
    dry_run: bool,
    yes: bool,
    home: &Path,
) -> Result<InstallOutcome> {
    let root = is_root();
    let tty = io::stdin().is_terminal() && io::stdout().is_terminal();
    let sudo_n_ok = if !root && yes {
        passwordless_sudo() || sudo_cached()
    } else {
        false
    };
    let can_sudo = sudo_required(root, yes, sudo_n_ok, tty);
    let plan = plan_install_with_home(paths, admin_token, start, home)?;

    if dry_run {
        print_plan(&plan, root);
        return Ok(InstallOutcome {
            unit_path: plan.unit_path,
            env_path: None,
            skipped: false,
        });
    }

    let staging = staging_dir_with_home(home, paths.role);
    create_private_dir(&staging)?;
    write_private(&staging.join(&plan.unit_name), plan.unit.as_bytes())?;

    let mut skipped = false;
    for spec in &plan.commands {
        if !run_command(spec, root, can_sudo, yes)? {
            skipped = true;
        }
    }

    if skipped {
        print_plan(&plan, root);
    }

    Ok(InstallOutcome {
        unit_path: plan.unit_path,
        env_path: None,
        skipped,
    })
}

/// Disable the unit, remove it, and reload systemd. Configuration, databases,
/// keys, and tokens are left untouched.
pub fn uninstall(paths: &RolePaths, dry_run: bool, yes: bool) -> Result<()> {
    let root = is_root();
    let tty = io::stdin().is_terminal() && io::stdout().is_terminal();
    let sudo_n_ok = if !root && yes {
        passwordless_sudo() || sudo_cached()
    } else {
        false
    };
    let can_sudo = sudo_required(root, yes, sudo_n_ok, tty);
    let unit_name = paths.role.unit_name().to_string();
    let unit_path = unit_path(paths).display().to_string();

    let commands = vec![
        CommandSpec::new(
            "systemctl",
            vec!["disable".into(), "--now".into(), unit_name],
            true,
        ),
        CommandSpec::new("rm", vec!["-f".into(), unit_path], true),
        CommandSpec::new("systemctl", vec!["daemon-reload".into()], true),
    ];

    if dry_run {
        for spec in &commands {
            println!("{}", spec.display(root));
        }
        return Ok(());
    }

    if !can_sudo {
        eprintln!(
            "service removal needs sudo; run these commands:\n  {}",
            commands
                .iter()
                .map(|spec| spec.display(false))
                .collect::<Vec<_>>()
                .join("\n  ")
        );
        return Ok(());
    }

    // `disable --now` fails when the unit is not installed; removal is still
    // safe and idempotent.
    let _ = run_command(&commands[0], root, can_sudo, yes);
    for spec in &commands[1..] {
        run_command(spec, root, can_sudo, yes)?;
    }
    Ok(())
}

fn print_plan(plan: &ServicePlan, root: bool) {
    println!("{}", plan.unit.trim_end());
    println!("\n# commands:");
    for spec in &plan.commands {
        println!("{}", spec.display(root));
    }
}

fn staging_dir_with_home(home: &Path, role: Role) -> PathBuf {
    home.join(".fleet/staging").join(role.as_str())
}

fn passwordless_sudo() -> bool {
    Command::new("sudo")
        .args(["-n", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// True when `fleet install` authenticated sudo for this process tree before
/// launching the setup subprocess. The credential is cached, so a later
/// `sudo -n` call succeeds even though passwordless sudo is not configured.
pub(crate) fn sudo_cached() -> bool {
    std::env::var_os("FLEET_SUDO_CACHED").is_some_and(|value| !value.is_empty())
}

/// Run one planned command. Returns false when the command was skipped because
/// sudo cannot be used (no root, no TTY, and no `--yes`).
fn run_command(spec: &CommandSpec, root: bool, can_sudo: bool, yes: bool) -> Result<bool> {
    if !spec.privileged || root {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        run_status(&mut command, &spec.display(root))?;
        return Ok(true);
    }

    if !can_sudo {
        eprintln!(
            "skipping privileged command (run with sudo): {}",
            spec.display(false)
        );
        return Ok(false);
    }

    let mut command = Command::new("sudo");
    if yes {
        // `--yes` is non-interactive; never let sudo prompt.
        command.arg("-n");
    }
    command.arg(&spec.program).args(&spec.args);
    run_status(&mut command, &spec.display(root))?;
    Ok(true)
}

fn run_status(command: &mut Command, display: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to run `{display}`"))?;
    if !status.success() {
        bail!("`{display}` exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(role: Role, home: &Path) -> RolePaths {
        RolePaths {
            role,
            config: home.join(".fleet").join(role.as_str()).join("config.toml"),
            state_dir: home.join(".fleet").join(role.as_str()),
            database: PathBuf::new(),
            binary: None,
            fleet_binary: PathBuf::from("/Users/dev/.local/bin/fleet"),
            service_user: "dev".to_string(),
            service_group: "dev".to_string(),
            web_dir: None,
        }
    }

    #[test]
    fn renders_admin_unit_as_the_invoking_user() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let paths = test_paths(Role::Admin, &home);
        let unit = render_unit_with_home(&paths, &home).unwrap();

        assert!(unit.contains("User=dev"));
        assert!(unit.contains("Group=dev"));
        assert!(unit.contains(&format!("Environment=HOME={}", home.display())));
        assert!(!unit.contains("EnvironmentFile="));
        assert!(!unit.contains("ProtectHome=true"));
        assert!(unit.contains(&format!(
            "ExecStart=/Users/dev/.local/bin/fleet admin run --config {}",
            paths.config.display()
        )));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("ReadWritePaths="));
        assert!(!unit.contains('@'));
        assert!(!unit.contains("WEB_DIR"));

        let limit = unit.find("StartLimitIntervalSec=60").unwrap();
        let service = unit.find("[Service]").unwrap();
        assert!(limit < service, "StartLimit* must live in [Unit]");
    }

    #[test]
    fn renders_daemon_unit_without_privileged_env_or_sandboxing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let paths = test_paths(Role::Daemon, &home);
        let unit = render_unit_with_home(&paths, &home).unwrap();

        assert!(unit.contains("User=dev"));
        assert!(unit.contains(&format!("Environment=HOME={}", home.display())));
        assert!(unit.contains(&format!(
            "ExecStart=/Users/dev/.local/bin/fleet daemon run --config {}",
            paths.config.display()
        )));
        assert!(!unit.contains("EnvironmentFile="));
        assert!(!unit.contains("ProtectHome=true"));
        assert!(!unit.contains("ProtectSystem="));
        assert!(!unit.contains("@ENV_FILE@"));
    }

    #[test]
    fn sudo_required_matrix() {
        assert!(!sudo_required(true, true, true, true));
        assert!(sudo_required(false, true, true, false));
        assert!(!sudo_required(false, true, false, true));
        assert!(sudo_required(false, false, false, true));
        assert!(!sudo_required(false, false, false, false));
        assert!(!sudo_required(false, false, true, false));
    }

    #[test]
    fn plan_installs_a_system_unit_for_the_invoking_user() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let paths = test_paths(Role::Admin, &home);
        let plan = plan_install_with_home(&paths, None, true, &home).unwrap();

        assert!(plan.env_path.is_none());
        assert!(!plan.commands.iter().any(|spec| spec.program == "useradd"));
        assert!(!plan.commands.iter().any(|spec| spec.program == "chown"));
        assert!(!plan.commands.iter().any(|spec| spec.program == "chmod"));
        assert!(plan
            .commands
            .iter()
            .any(|spec| spec.program == "install" && spec.args.iter().any(|arg| arg == "0644")));
        assert!(plan.commands.iter().any(|spec| {
            spec.program == "systemctl"
                && !spec.args.contains(&"--user".to_string())
                && spec.args.contains(&"--now".to_string())
        }));
        assert_eq!(
            plan.unit_path,
            std::path::PathBuf::from("/etc/systemd/system/fleet-admin.service")
        );
        assert!(plan.commands.iter().all(|spec| spec.privileged));
        assert!(plan
            .commands
            .iter()
            .any(|spec| spec.args.iter().any(|arg| arg
                == staging_dir_with_home(&home, Role::Admin)
                    .join(Role::Admin.unit_name())
                    .to_str()
                    .unwrap())));
    }

    #[test]
    fn plan_without_start_omits_now() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let paths = test_paths(Role::Daemon, &home);
        let plan = plan_install_with_home(&paths, None, false, &home).unwrap();

        assert!(plan.commands.iter().any(|spec| {
            spec.program == "systemctl"
                && spec.args.contains(&"enable".to_string())
                && !spec.args.contains(&"--now".to_string())
        }));
        assert!(plan.commands.iter().all(|spec| spec.privileged));
    }

    fn fake_tool(dir: &Path, body: &str) -> PathBuf {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-systemctl");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        wait_until_executable(&path);
        path
    }

    /// Another test thread can fork while this script is still open for
    /// writing, so `exec` fails with `ETXTBSY` ("Text file busy") until the
    /// inherited descriptor closes. Probe the fixture once and retry.
    fn wait_until_executable(path: &Path) {
        for attempt in 0..20 {
            match Command::new(path).output() {
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    if attempt == 19 {
                        panic!("fake systemctl stayed busy: {error}");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                _ => return,
            }
        }
    }

    #[test]
    fn systemctl_actions_invoke_the_expected_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let tool = fake_tool(dir.path(), "printf '%s\\n' \"$@\"");

        let output = action_output_at(&tool, "fleet-admin.service", "start").unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "start\nfleet-admin.service\n"
        );
    }

    #[test]
    fn systemctl_failure_prints_the_sudo_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = fake_tool(dir.path(), "echo 'access denied' >&2\nexit 1");

        let error = run_action_at(&tool, "fleet-daemon.service", "restart").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("access denied"), "{text}");
        assert!(
            text.contains("sudo systemctl restart fleet-daemon.service"),
            "{text}"
        );
    }

    #[test]
    fn status_reports_active_and_unit_policy() {
        let dir = tempfile::tempdir().unwrap();
        let tool = fake_tool(
            dir.path(),
            r#"if [ "$1" = "is-active" ]; then exit 0; fi
case "$3" in
  UnitFileState) echo enabled ;;
  Restart) echo always ;;
esac"#,
        );

        let status = status_at(&tool, "fleet-admin.service");
        assert!(status.active);
        assert_eq!(status.unit_file_state, "enabled");
        assert_eq!(status.restart, "always");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let paths = test_paths(Role::Admin, &home);

        let outcome = install_with_home(&paths, None, true, true, false, &home).unwrap();

        assert!(!outcome.skipped);
        assert!(!home.join(".fleet/staging").exists());
        assert!(!paths.config.exists());
        assert!(!paths.state_dir.exists());
    }
}

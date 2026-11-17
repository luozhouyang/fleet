//! `fleet install` — the single server-profile installer.
//!
//! Fleet runs exactly one deployment model: configuration and state live in the
//! invoking user's home, while the service definitions live in the system
//! service manager and run as that user. This command is the only installer
//! entry point; the shell/PowerShell scripts only download and verify a release
//! archive, then hand the extracted directory to `fleet install --from-dir`.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::config::{home_dir, write_private};
use crate::config_paths::{is_root, Role, RolePathOverrides, RolePaths};
use crate::process::{health, service};

/// Admin listener port used when `--port` is not supplied.
pub const DEFAULT_PORT: u16 = 8787;

#[derive(Debug, Clone, Args)]
pub struct InstallArgs {
    /// Directory containing the extracted release archive (`bin/`, `web/`).
    #[arg(long, required_unless_present = "repair")]
    pub from_dir: Option<PathBuf>,
    /// Install prefix (default: ~/.local).
    #[arg(long)]
    pub prefix: Option<PathBuf>,
    /// Admin loopback port (default: 8787, or the port from the manifest).
    #[arg(long)]
    pub port: Option<u16>,
    /// Never prompt for sudo; requires passwordless sudo.
    #[arg(long)]
    pub yes: bool,
    /// Copy files and write configuration only; do not install services.
    #[arg(long, hide = true)]
    pub no_service: bool,
    /// Replace existing binaries.
    #[arg(long)]
    pub force: bool,
    /// Print the plan without writing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Reinstall the system services and watchdog for an existing prefix.
    #[arg(long, conflicts_with = "from_dir")]
    pub repair: bool,
    /// Internal: this process was relaunched with elevation.
    #[arg(long, hide = true)]
    pub elevated: bool,
}

/// Manifest written next to the installed web assets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallManifest {
    pub schema: u32,
    pub version: String,
    pub user: String,
    pub home: PathBuf,
    pub prefix: PathBuf,
    pub data_root: PathBuf,
    pub admin_port: u16,
    pub files: Vec<PathBuf>,
    pub services: Vec<PathBuf>,
    pub installed_at: String,
}

pub async fn run(args: InstallArgs) -> Result<()> {
    if is_root() {
        bail!(
            "run `fleet install` as your normal user; Fleet asks for elevation only for the fixed service-definition commands"
        );
    }

    #[cfg(windows)]
    if !args.no_service && !args.dry_run && !args.elevated && !is_elevated()? {
        return relaunch_elevated();
    }

    if args.repair {
        return repair(&args).await;
    }

    let from_path = args
        .from_dir
        .as_ref()
        .context("--from-dir is required unless --repair is used")?;
    let from_dir = from_path
        .canonicalize()
        .with_context(|| format!("install source does not exist: {}", from_path.display()))?;
    let prefix = absolute_prefix(args.prefix.clone())?;
    let port = args.port.unwrap_or(DEFAULT_PORT);
    let bin_dir = prefix.join("bin");
    let web_dir = prefix.join("share").join("fleet").join("web");

    if !args.no_service {
        ensure_user_owns_data_root()?;
    }

    let binaries = binary_names();
    for name in binaries {
        let source = from_dir.join("bin").join(name);
        if !source.is_file() {
            bail!(
                "release archive is missing bin/{name}: {}",
                source.display()
            );
        }
    }

    if !args.force {
        for name in binaries {
            let target = bin_dir.join(name);
            if target.exists() {
                bail!(
                    "{} already exists; pass --force to reinstall",
                    target.display()
                );
            }
        }
    }

    if !args.dry_run && !args.force && !args.no_service && !port_available(port) {
        bail!("127.0.0.1:{port} is already in use; stop the existing admin or pass --port <PORT>");
    }

    print_plan(
        &from_dir,
        &prefix,
        &bin_dir,
        &web_dir,
        port,
        args.no_service,
    );

    if args.dry_run {
        return Ok(());
    }

    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;
    for name in binaries {
        replace_file(&from_dir.join("bin").join(name), &bin_dir.join(name))?;
    }

    let web_source = from_dir.join("web");
    if web_source.is_dir() {
        if web_dir.exists() {
            fs::remove_dir_all(&web_dir)
                .with_context(|| format!("failed to replace {}", web_dir.display()))?;
        }
        copy_tree(&web_source, &web_dir)?;
    } else if args.no_service {
        eprintln!("warning: release archive has no web/ directory; the admin will need --web-dir");
    } else {
        bail!(
            "release archive is missing web/; cannot install the admin UI (use --no-service to copy files only)"
        );
    }

    let fleet_bin = bin_dir.join(binaries[0]);
    if args.no_service {
        let manifest = manifest(&prefix, &web_dir, port, &binaries, true);
        write_manifest(&prefix, &manifest)?;
        println!("\nFleet files installed; no service was configured (--no-service).");
        return Ok(());
    }

    prepare_sudo(args.yes)?;
    run_admin_setup(&fleet_bin, port, &web_dir)?;
    run_daemon_setup(&fleet_bin, port)?;

    let admin = RolePaths::resolve_config(Role::Admin, &RolePathOverrides::default())?;
    let daemon = RolePaths::resolve_config(Role::Daemon, &RolePathOverrides::default())?;
    for (label, paths) in [("admin", &admin), ("daemon", &daemon)] {
        if !service::installed(paths) {
            bail!(
                "the {label} service definition was not installed; run the sudo commands printed above, then `fleet status`"
            );
        }
    }

    install_watchdog(&prefix, port)?;

    wait_ready("admin", &admin_ready_url(port), Duration::from_secs(30)).await?;
    wait_healthy("daemon", &daemon, Duration::from_secs(30)).await?;

    let manifest = manifest(&prefix, &web_dir, port, &binaries, false);
    write_manifest(&prefix, &manifest)?;
    print_summary(&prefix, &web_dir, port);
    Ok(())
}

/// Reinstall the service definitions and the watchdog for an existing prefix.
async fn repair(args: &InstallArgs) -> Result<()> {
    ensure_user_owns_data_root()?;
    let prefix = absolute_prefix(args.prefix.clone())?;
    let port = existing_port(&prefix).unwrap_or(DEFAULT_PORT);
    let names = binary_names();
    let fleet_bin = prefix.join("bin").join(names[0]);
    if !fleet_bin.is_file() {
        bail!(
            "{} is missing; run the install script without --repair",
            fleet_bin.display()
        );
    }
    for name in &names[1..] {
        let path = prefix.join("bin").join(name);
        if !path.is_file() {
            bail!(
                "{} is missing; run the install script without --repair",
                path.display()
            );
        }
    }

    println!("Fleet installer repair");
    println!("  prefix    {}", prefix.display());
    println!("  admin     http://127.0.0.1:{port}");

    prepare_sudo(args.yes)?;

    let mut daemon: Option<RolePaths> = None;
    for role in [Role::Admin, Role::Daemon] {
        let mut paths = RolePaths::resolve_config(role, &RolePathOverrides::default())?;
        if !paths.config.is_file() {
            bail!(
                "{} is missing; run the install script without --repair",
                paths.config.display()
            );
        }
        paths.fleet_binary = fleet_bin.clone();
        paths.binary = Some(prefix.join("bin").join(match role {
            Role::Admin => names[1],
            Role::Daemon => names[2],
        }));
        let outcome = service::install(&paths, None, true, false, args.yes)?;
        if outcome.skipped {
            bail!(
                "the {} service definition was not installed; rerun `fleet install --repair` from a terminal or with passwordless sudo",
                role.as_str()
            );
        }
        if role == Role::Daemon {
            daemon = Some(paths);
        }
    }

    install_watchdog(&prefix, port)?;

    wait_ready("admin", &admin_ready_url(port), Duration::from_secs(30)).await?;
    let daemon = daemon.context("daemon paths were not resolved")?;
    wait_healthy("daemon", &daemon, Duration::from_secs(30)).await?;

    let web_dir = prefix.join("share").join("fleet").join("web");
    let binaries = binary_names();
    let manifest = manifest(&prefix, &web_dir, port, &binaries, false);
    write_manifest(&prefix, &manifest)?;
    print_summary(&prefix, &web_dir, port);
    Ok(())
}

/// Refuse to install over a data root that root created for another user.
fn ensure_user_owns_data_root() -> Result<()> {
    let root = home_dir().join(".fleet");
    if !root.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata =
            fs::metadata(&root).with_context(|| format!("failed to inspect {}", root.display()))?;
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid {
            bail!(
                "{} is owned by uid {}; run: sudo chown -R \"$(id -u):$(id -g)\" {}",
                root.display(),
                metadata.uid(),
                root.display()
            );
        }
        if metadata.mode() & 0o077 != 0 {
            bail!(
                "{} is accessible by group or others; run: chmod 700 {}",
                root.display(),
                root.display()
            );
        }
    }
    Ok(())
}

fn existing_port(prefix: &Path) -> Option<u16> {
    let admin = RolePaths::resolve_config(Role::Admin, &RolePathOverrides::default()).ok()?;
    if admin.config.is_file() {
        if let Ok(config) = fleet_core_config::AdminConfig::load(Some(&admin.config)) {
            return Some(config.server.port);
        }
    }
    let raw = fs::read_to_string(prefix.join("share").join("fleet").join("install.json")).ok()?;
    serde_json::from_str::<InstallManifest>(&raw)
        .ok()
        .map(|manifest| manifest.admin_port)
}

fn admin_ready_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/api/v1/ready")
}

fn absolute_prefix(prefix: Option<PathBuf>) -> Result<PathBuf> {
    let prefix = prefix.unwrap_or_else(default_prefix);
    if prefix.is_absolute() {
        Ok(prefix)
    } else {
        Ok(std::env::current_dir()?.join(prefix))
    }
}

fn default_prefix() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData").join("Local"))
            .join("Fleet")
    }
    #[cfg(not(windows))]
    {
        home_dir().join(".local")
    }
}

fn binary_names() -> [&'static str; 3] {
    if cfg!(windows) {
        ["fleet.exe", "fleet-admin.exe", "fleet-daemon.exe"]
    } else {
        ["fleet", "fleet-admin", "fleet-daemon"]
    }
}

fn port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn print_plan(
    from_dir: &Path,
    prefix: &Path,
    bin_dir: &Path,
    web_dir: &Path,
    port: u16,
    no_service: bool,
) {
    println!("Fleet installer plan");
    println!("  source    {}", from_dir.display());
    println!("  prefix    {}", prefix.display());
    println!("  binaries  {}", bin_dir.display());
    println!("  web       {}", web_dir.display());
    println!("  admin     http://127.0.0.1:{port}");
    println!("  services  {}", if no_service { "skip" } else { "system" });
    if !no_service {
        println!("  watchdog  system, every 30s");
    }
}

fn replace_file(source: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .context("install target has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temp = parent.join(format!(
        ".{}.new.{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fleet"),
        std::process::id()
    ));
    fs::copy(source, &temp).with_context(|| format!("failed to copy {}", source.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to set permissions on {}", temp.display()))?;
    }
    if target.exists() {
        let old = target.with_extension("old");
        let _ = fs::remove_file(&old);
        fs::rename(target, &old)
            .with_context(|| format!("failed to move {} aside", target.display()))?;
    }
    fs::rename(&temp, target).with_context(|| format!("failed to install {}", target.display()))?;
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), &destination)
                .with_context(|| format!("failed to copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Authenticate sudo once so the setup subprocesses can use `sudo -n` for the
/// fixed service-definition commands.
fn prepare_sudo(non_interactive: bool) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = non_interactive;
        if !is_elevated()? {
            bail!(
                "Task Scheduler registration requires elevation; rerun the installer and accept the UAC prompt"
            );
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sudo");
        if non_interactive {
            command.arg("-n").arg("true");
        } else {
            command.arg("-v");
        }
        let status = command
            .status()
            .context("failed to run sudo; rerun without --yes to allow one password prompt")?;
        if !status.success() {
            if non_interactive {
                bail!("--yes requires passwordless sudo; rerun without --yes for a single prompt");
            }
            bail!("sudo authentication failed; the service definitions cannot be installed");
        }
        Ok(())
    }
}

/// Relaunch the installer through UAC once, then propagate the child's exit
/// code. User files are written by the elevated process under the same account.
#[cfg(windows)]
fn relaunch_elevated() -> Result<()> {
    let exe = std::env::current_exe().context("could not locate the running fleet binary")?;
    let cwd = std::env::current_dir().context("could not determine the current directory")?;
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    args.push("--elevated".to_string());
    if !args.iter().any(|arg| arg == "--force") {
        args.push("--force".to_string());
    }
    let arg_list = args
        .iter()
        .map(|arg| ps_quote(arg))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "$p = Start-Process -FilePath {} -WorkingDirectory {} -ArgumentList {} -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
        ps_quote(&exe.display().to_string()),
        ps_quote(&cwd.display().to_string()),
        arg_list
    );
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status()
        .context("failed to start the elevated installer")?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn is_elevated() -> Result<bool> {
    let script = "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("failed to determine whether Fleet is elevated")?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .eq_ignore_ascii_case("true"))
}

fn run_admin_setup(fleet_bin: &Path, port: u16, web_dir: &Path) -> Result<()> {
    let bind = format!("127.0.0.1:{port}");
    let mut command = Command::new(fleet_bin);
    command.env("FLEET_SUDO_CACHED", "1");
    command.args([
        "admin",
        "setup",
        "--yes",
        "--start",
        "--install-service",
        "--bind",
        &bind,
    ]);
    if web_dir.is_dir() {
        command.arg("--web-dir").arg(web_dir);
    }
    run_status(command, "admin setup")
}

fn run_daemon_setup(fleet_bin: &Path, port: u16) -> Result<()> {
    let server = format!("ws://127.0.0.1:{port}/ws/machine");
    let mut command = Command::new(fleet_bin);
    command.env("FLEET_SUDO_CACHED", "1");
    command.args([
        "daemon",
        "setup",
        "--yes",
        "--start",
        "--install-service",
        "--org",
        "org_default",
        "--server",
        &server,
    ]);
    run_status(command, "daemon setup")
}

fn run_status(mut command: Command, label: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to run {label}"))?;
    if !status.success() {
        bail!("{label} failed with {status}; inspect the output above");
    }
    Ok(())
}

async fn wait_healthy(label: &str, paths: &RolePaths, limit: Duration) -> Result<()> {
    let url = health::health_url(paths)?;
    wait_ready(label, &url, limit).await
}

async fn wait_ready(label: &str, url: &str, limit: Duration) -> Result<()> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if health::probe_url(url).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!(
        "{label} did not become ready at {url} within {}s; inspect `fleet {label} logs`",
        limit.as_secs()
    )
}

fn manifest(
    prefix: &Path,
    web_dir: &Path,
    port: u16,
    binaries: &[&str],
    no_service: bool,
) -> InstallManifest {
    let bin_dir = prefix.join("bin");
    let mut files: Vec<PathBuf> = binaries.iter().map(|name| bin_dir.join(name)).collect();
    files.push(web_dir.to_path_buf());
    InstallManifest {
        schema: 1,
        version: env!("CARGO_PKG_VERSION").to_string(),
        user: crate::config_paths::current_user(),
        home: home_dir(),
        prefix: prefix.to_path_buf(),
        data_root: home_dir().join(".fleet"),
        admin_port: port,
        files,
        services: system_paths(no_service),
        installed_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn system_paths(no_service: bool) -> Vec<PathBuf> {
    if no_service {
        return Vec::new();
    }
    let mut paths = service_paths();
    paths.extend(watchdog_system_paths());
    paths
}

fn service_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/Library/LaunchDaemons/dev.fleet.admin.plist"),
            PathBuf::from("/Library/LaunchDaemons/dev.fleet.daemon.plist"),
        ]
    }
    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from("/etc/systemd/system/fleet-admin.service"),
            PathBuf::from("/etc/systemd/system/fleet-daemon.service"),
        ]
    }
    #[cfg(windows)]
    {
        vec![PathBuf::from("fleet-admin"), PathBuf::from("fleet-daemon")]
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Watchdog
// ---------------------------------------------------------------------------

#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
const WATCHDOG_SCRIPT_TEMPLATE: &str =
    include_str!("../../../packaging/watchdog/fleet-watchdog.sh");
#[cfg(target_os = "macos")]
const WATCHDOG_PLIST_TEMPLATE: &str =
    include_str!("../../../packaging/launchd/dev.fleet.watchdog.plist.in");
#[cfg(target_os = "linux")]
const WATCHDOG_SERVICE_TEMPLATE: &str =
    include_str!("../../../packaging/systemd/fleet-watchdog.service.in");
#[cfg(target_os = "linux")]
const WATCHDOG_TIMER_TEMPLATE: &str =
    include_str!("../../../packaging/systemd/fleet-watchdog.timer.in");

#[cfg(any(target_os = "macos", target_os = "linux"))]
const WATCHDOG_SCRIPT_PATH: &str = "/usr/local/libexec/fleet-watchdog";

#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
struct WatchdogValues {
    admin_url: String,
    daemon_url: String,
    manager: &'static str,
    admin_id: &'static str,
    daemon_id: &'static str,
    state_dir: &'static str,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn watchdog_values(port: u16, daemon_url: &str) -> WatchdogValues {
    #[cfg(target_os = "macos")]
    let (manager, admin_id, daemon_id, state_dir) = (
        "launchd",
        "dev.fleet.admin",
        "dev.fleet.daemon",
        "/var/db/fleet-watchdog",
    );
    #[cfg(target_os = "linux")]
    let (manager, admin_id, daemon_id, state_dir) = (
        "systemd",
        "fleet-admin.service",
        "fleet-daemon.service",
        "/var/lib/fleet-watchdog",
    );
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let (manager, admin_id, daemon_id, state_dir) = (
        "systemd",
        "fleet-admin.service",
        "fleet-daemon.service",
        "/var/lib/fleet-watchdog",
    );
    WatchdogValues {
        admin_url: admin_ready_url(port),
        daemon_url: daemon_url.to_string(),
        manager,
        admin_id,
        daemon_id,
        state_dir,
    }
}

#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn validate_watchdog_values(values: &WatchdogValues) -> Result<()> {
    let entries = [
        ("admin health URL", values.admin_url.as_str()),
        ("daemon health URL", values.daemon_url.as_str()),
        ("service manager", values.manager),
        ("admin service id", values.admin_id),
        ("daemon service id", values.daemon_id),
        ("state directory", values.state_dir),
    ];
    for (label, value) in entries {
        if value.is_empty()
            || !value.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '/' | ':' | '.' | '_' | '-' | '@' | '+')
            })
        {
            bail!("refusing to install the watchdog with an unsafe {label}: {value:?}");
        }
    }
    Ok(())
}

/// Substitute the shell assignments by name. This avoids depending on the
/// placeholder spelling used by the template.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn render_watchdog_script(template: &str, values: &WatchdogValues) -> String {
    let assignments = [
        ("ADMIN_HEALTH_URL", values.admin_url.as_str()),
        ("DAEMON_HEALTH_URL", values.daemon_url.as_str()),
        ("SERVICE_MANAGER", values.manager),
        ("ADMIN_SERVICE_ID", values.admin_id),
        ("DAEMON_SERVICE_ID", values.daemon_id),
        ("STATE_DIR", values.state_dir),
    ];
    let mut output = String::with_capacity(template.len() + 256);
    for line in template.lines() {
        let rendered = assignments.iter().find_map(|(name, value)| {
            let rest = line.strip_prefix(name)?;
            rest.starts_with('=').then(|| format!("{name}='{value}'"))
        });
        output.push_str(rendered.as_deref().unwrap_or(line));
        output.push('\n');
    }
    output
}

#[cfg(target_os = "macos")]
fn render_watchdog_plist(template: &str, script: &Path) -> Result<String> {
    let mut replaced = false;
    let mut output = String::with_capacity(template.len() + 128);
    for line in template.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<string>@") && trimmed.ends_with("</string>") {
            replaced = true;
            output.push_str(&format!("    <string>{}</string>", script.display()));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    if !replaced {
        bail!("watchdog launchd template has no script placeholder");
    }
    Ok(output)
}

#[cfg(target_os = "linux")]
fn render_watchdog_unit(template: &str, script: &Path) -> Result<String> {
    let mut replaced = false;
    let mut output = String::with_capacity(template.len() + 128);
    for line in template.lines() {
        if line.starts_with("ExecStart=") {
            replaced = true;
            output.push_str(&format!("ExecStart={}", script.display()));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    if !replaced {
        bail!("watchdog systemd template has no ExecStart line");
    }
    Ok(output)
}

fn install_watchdog(prefix: &Path, port: u16) -> Result<Vec<PathBuf>> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let daemon = RolePaths::resolve_config(Role::Daemon, &RolePathOverrides::default())?;
        let daemon_url = health::health_url(&daemon)?;
        let values = watchdog_values(port, &daemon_url);
        validate_watchdog_values(&values)?;

        let stage = prefix.join("share").join("fleet").join("watchdog");
        fs::create_dir_all(&stage)
            .with_context(|| format!("failed to create {}", stage.display()))?;
        let script = render_watchdog_script(WATCHDOG_SCRIPT_TEMPLATE, &values);
        let staged_script = stage.join("fleet-watchdog.sh");
        write_executable(&staged_script, &script)?;

        #[cfg(target_os = "macos")]
        {
            install_watchdog_launchd(&stage, &staged_script)
        }
        #[cfg(target_os = "linux")]
        {
            install_watchdog_systemd(&stage, &staged_script)
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (prefix, port);
        eprintln!(
            "note: the Windows watchdog task is not installed in this build; Task Scheduler restart-on-failure remains active"
        );
        Ok(Vec::new())
    }
}

#[cfg(target_os = "macos")]
fn install_watchdog_launchd(stage: &Path, staged_script: &Path) -> Result<Vec<PathBuf>> {
    use std::process::Stdio;

    const PLIST: &str = "/Library/LaunchDaemons/dev.fleet.watchdog.plist";
    const LABEL: &str = "system/dev.fleet.watchdog";

    let plist = render_watchdog_plist(WATCHDOG_PLIST_TEMPLATE, Path::new(WATCHDOG_SCRIPT_PATH))?;
    let staged_plist = stage.join("dev.fleet.watchdog.plist");
    fs::write(&staged_plist, plist)
        .with_context(|| format!("failed to write {}", staged_plist.display()))?;

    sudo_fixed(
        "/usr/bin/install",
        &[
            "-d",
            "-m",
            "0700",
            "-o",
            "root",
            "-g",
            "wheel",
            "/var/db/fleet-watchdog",
        ],
    )?;
    sudo_fixed("/usr/sbin/chown", &["root:wheel", "/var/db/fleet-watchdog"])?;
    sudo_fixed("/bin/chmod", &["0700", "/var/db/fleet-watchdog"])?;
    // /usr/local/libexec is not guaranteed to exist (notably on macOS), and
    // macOS install(1) creates its temporary file next to the destination.
    sudo_fixed(
        "/usr/bin/install",
        &[
            "-d",
            "-m",
            "0755",
            "-o",
            "root",
            "-g",
            "wheel",
            "/usr/local/libexec",
        ],
    )?;
    sudo_fixed(
        "/usr/bin/install",
        &[
            "-m",
            "0755",
            "-o",
            "root",
            "-g",
            "wheel",
            &staged_script.display().to_string(),
            WATCHDOG_SCRIPT_PATH,
        ],
    )?;
    sudo_fixed(
        "/usr/bin/install",
        &[
            "-m",
            "0644",
            "-o",
            "root",
            "-g",
            "wheel",
            &staged_plist.display().to_string(),
            PLIST,
        ],
    )?;
    let _ = Command::new("sudo")
        .args(["-n", "/bin/launchctl", "bootout", LABEL])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    sudo_fixed("/bin/launchctl", &["bootstrap", "system", PLIST])?;

    verify_system_file(Path::new(WATCHDOG_SCRIPT_PATH))?;
    verify_system_file(Path::new(PLIST))?;
    Ok(vec![
        PathBuf::from(WATCHDOG_SCRIPT_PATH),
        PathBuf::from(PLIST),
    ])
}

#[cfg(target_os = "linux")]
fn install_watchdog_systemd(stage: &Path, staged_script: &Path) -> Result<Vec<PathBuf>> {
    const SERVICE: &str = "/etc/systemd/system/fleet-watchdog.service";
    const TIMER: &str = "/etc/systemd/system/fleet-watchdog.timer";

    let service = render_watchdog_unit(WATCHDOG_SERVICE_TEMPLATE, Path::new(WATCHDOG_SCRIPT_PATH))?;
    let staged_service = stage.join("fleet-watchdog.service");
    fs::write(&staged_service, service)
        .with_context(|| format!("failed to write {}", staged_service.display()))?;
    let staged_timer = stage.join("fleet-watchdog.timer");
    fs::write(&staged_timer, WATCHDOG_TIMER_TEMPLATE)
        .with_context(|| format!("failed to write {}", staged_timer.display()))?;

    sudo_fixed(
        "/usr/bin/install",
        &[
            "-d",
            "-m",
            "0700",
            "-o",
            "root",
            "-g",
            "root",
            "/var/lib/fleet-watchdog",
        ],
    )?;
    sudo_fixed("/usr/bin/chown", &["root:root", "/var/lib/fleet-watchdog"])?;
    sudo_fixed("/usr/bin/chmod", &["0700", "/var/lib/fleet-watchdog"])?;
    // The watchdog script lives in /usr/local/libexec; not every distribution
    // ships that directory.
    sudo_fixed(
        "/usr/bin/install",
        &[
            "-d",
            "-m",
            "0755",
            "-o",
            "root",
            "-g",
            "root",
            "/usr/local/libexec",
        ],
    )?;
    sudo_fixed(
        "/usr/bin/install",
        &[
            "-m",
            "0755",
            "-o",
            "root",
            "-g",
            "root",
            &staged_script.display().to_string(),
            WATCHDOG_SCRIPT_PATH,
        ],
    )?;
    sudo_fixed(
        "/usr/bin/install",
        &[
            "-m",
            "0644",
            "-o",
            "root",
            "-g",
            "root",
            &staged_service.display().to_string(),
            SERVICE,
        ],
    )?;
    sudo_fixed(
        "/usr/bin/install",
        &[
            "-m",
            "0644",
            "-o",
            "root",
            "-g",
            "root",
            &staged_timer.display().to_string(),
            TIMER,
        ],
    )?;
    sudo_fixed("/usr/bin/systemctl", &["daemon-reload"])?;
    sudo_fixed(
        "/usr/bin/systemctl",
        &["enable", "--now", "fleet-watchdog.timer"],
    )?;

    verify_system_file(Path::new(WATCHDOG_SCRIPT_PATH))?;
    verify_system_file(Path::new(SERVICE))?;
    verify_system_file(Path::new(TIMER))?;
    Ok(vec![
        PathBuf::from(WATCHDOG_SCRIPT_PATH),
        PathBuf::from(SERVICE),
        PathBuf::from(TIMER),
    ])
}

pub(crate) fn watchdog_system_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from(WATCHDOG_SCRIPT_PATH),
            PathBuf::from("/Library/LaunchDaemons/dev.fleet.watchdog.plist"),
        ]
    }
    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from(WATCHDOG_SCRIPT_PATH),
            PathBuf::from("/etc/systemd/system/fleet-watchdog.service"),
            PathBuf::from("/etc/systemd/system/fleet-watchdog.timer"),
        ]
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Vec::new()
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_executable(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }
    Ok(())
}

/// Run a fixed administrative command with the sudo credential cached by
/// `prepare_sudo`. Never used for a user-writable binary.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn sudo_fixed(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new("sudo")
        .arg("-n")
        .arg(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run sudo {program}"))?;
    if !status.success() {
        bail!(
            "sudo {program} {} failed; rerun `fleet install --repair` from a terminal",
            args.join(" ")
        );
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn verify_system_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        bail!(
            "expected {} to exist after the watchdog install; rerun with a terminal so sudo can prompt",
            path.display()
        )
    }
}

fn write_manifest(prefix: &Path, manifest: &InstallManifest) -> Result<()> {
    let dir = prefix.join("share").join("fleet");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let raw = serde_json::to_vec_pretty(manifest).context("failed to serialize the manifest")?;
    write_private(&dir.join("install.json"), &raw)
        .with_context(|| format!("failed to write {}", dir.join("install.json").display()))
}

fn print_summary(prefix: &Path, web_dir: &Path, port: u16) {
    println!();
    println!("Fleet {} installed", env!("CARGO_PKG_VERSION"));
    println!("  binaries  {}", prefix.join("bin").display());
    println!("  web       {}", web_dir.display());
    println!("  data      {}", home_dir().join(".fleet").display());
    println!("  admin     http://127.0.0.1:{port}");
    println!(
        "  services  system (running as {})",
        crate::config_paths::current_user()
    );
    println!("  watchdog  system, every 30s");
    println!();
    println!("Next:");
    println!("  fleet status");
    println!("  fleet admin config show --reveal");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_archive(root: &Path) -> PathBuf {
        let archive = root.join("unpack");
        fs::create_dir_all(archive.join("bin")).unwrap();
        fs::create_dir_all(archive.join("web")).unwrap();
        for name in binary_names() {
            fs::write(archive.join("bin").join(name), b"binary").unwrap();
        }
        fs::write(archive.join("web").join("index.html"), b"<html></html>").unwrap();
        archive
    }

    #[test]
    fn copy_tree_preserves_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = fake_archive(dir.path());
        let target = dir.path().join("web-copy");
        copy_tree(&source.join("web"), &target).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("index.html")).unwrap(),
            "<html></html>"
        );
    }

    #[test]
    fn replace_file_is_atomic_and_executable() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::write(&source, b"new").unwrap();
        let target = dir.path().join("prefix/bin/fleet");
        replace_file(&source, &target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn port_available_rejects_a_bound_port() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!port_available(port));
        drop(listener);
    }

    fn watchdog_values_fixture() -> WatchdogValues {
        WatchdogValues {
            admin_url: "http://127.0.0.1:8787/api/v1/ready".to_string(),
            daemon_url: "http://127.0.0.1:8788/health".to_string(),
            manager: "systemd",
            admin_id: "fleet-admin.service",
            daemon_id: "fleet-daemon.service",
            state_dir: "/var/lib/fleet-watchdog",
        }
    }

    #[test]
    fn watchdog_script_substitutes_every_assignment() {
        let rendered = render_watchdog_script(WATCHDOG_SCRIPT_TEMPLATE, &watchdog_values_fixture());
        assert!(rendered.contains("ADMIN_HEALTH_URL='http://127.0.0.1:8787/api/v1/ready'"));
        assert!(rendered.contains("DAEMON_HEALTH_URL='http://127.0.0.1:8788/health'"));
        assert!(rendered.contains("SERVICE_MANAGER='systemd'"));
        assert!(rendered.contains("ADMIN_SERVICE_ID='fleet-admin.service'"));
        assert!(rendered.contains("DAEMON_SERVICE_ID='fleet-daemon.service'"));
        assert!(rendered.contains("STATE_DIR='/var/lib/fleet-watchdog'"));
        for name in [
            "ADMIN_HEALTH_URL",
            "DAEMON_HEALTH_URL",
            "SERVICE_MANAGER",
            "ADMIN_SERVICE_ID",
            "DAEMON_SERVICE_ID",
            "STATE_DIR",
        ] {
            assert!(
                !rendered
                    .lines()
                    .any(|line| line.starts_with(&format!("{name}=@"))),
                "{name} placeholder survived rendering"
            );
        }
    }

    #[test]
    fn watchdog_values_reject_shell_metacharacters() {
        let mut values = watchdog_values_fixture();
        values.state_dir = "/var/lib/fleet;rm -rf /";
        assert!(validate_watchdog_values(&values).is_err());
        values.state_dir = "/var/lib/fleet-watchdog";
        assert!(validate_watchdog_values(&values).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn watchdog_plist_points_at_the_installed_script() {
        let rendered =
            render_watchdog_plist(WATCHDOG_PLIST_TEMPLATE, Path::new(WATCHDOG_SCRIPT_PATH))
                .unwrap();
        assert!(rendered.contains("<string>/bin/sh</string>"));
        assert!(rendered.contains(&format!("<string>{WATCHDOG_SCRIPT_PATH}</string>")));
        assert!(!rendered.contains("<string>@"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn watchdog_unit_points_at_the_installed_script() {
        let rendered =
            render_watchdog_unit(WATCHDOG_SERVICE_TEMPLATE, Path::new(WATCHDOG_SCRIPT_PATH))
                .unwrap();
        assert!(rendered.contains(&format!("ExecStart={WATCHDOG_SCRIPT_PATH}")));
        assert!(!rendered.contains("ExecStart=@"));
    }

    #[tokio::test]
    async fn no_service_install_copies_files_and_writes_a_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let archive = fake_archive(dir.path());
        let prefix = dir.path().join("prefix");
        run(InstallArgs {
            from_dir: Some(archive),
            prefix: Some(prefix.clone()),
            port: None,
            yes: false,
            no_service: true,
            force: false,
            dry_run: false,
            repair: false,
            elevated: false,
        })
        .await
        .unwrap();

        for name in binary_names() {
            assert!(prefix.join("bin").join(name).is_file());
        }
        assert!(prefix.join("share/fleet/web/index.html").is_file());
        let manifest: InstallManifest = serde_json::from_str(
            &fs::read_to_string(prefix.join("share/fleet/install.json")).unwrap(),
        )
        .unwrap();
        assert!(manifest.services.is_empty());
        assert_eq!(manifest.admin_port, DEFAULT_PORT);
    }
}

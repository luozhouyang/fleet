//! launchd LaunchAgent/LaunchDaemon backend for macOS.
//!
//! User scope writes `~/Library/LaunchAgents/<label>.plist`; system scope
//! writes `/Library/LaunchDaemons/<label>.plist` and requires root.
//!
//! The module is compiled on every host so its plist rendering can be tested
//! without a Mac; only the macOS build selects it at runtime.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config::home_dir;
use crate::config_paths::{is_root, RolePaths};

use super::service::InstallOutcome;

const DEFAULT_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Bundle identifier of the `Fleet.app` wrapper shipped in macOS releases.
///
/// The System Settings Login Items UI resolves a launchd job's display name
/// through its associated bundle. Without this association a bare executable
/// is labelled with the Developer ID certificate name instead of "Fleet".
pub const APP_BUNDLE_IDENTIFIER: &str = crate::macos_app::BUNDLE_IDENTIFIER;

pub fn launchd_available() -> bool {
    cfg!(target_os = "macos")
}

pub fn label(paths: &RolePaths) -> String {
    format!("dev.fleet.{}", paths.role.as_str())
}

pub fn plist_path(paths: &RolePaths) -> PathBuf {
    plist_path_with_home(paths, &home_dir())
}

fn plist_path_with_home(paths: &RolePaths, home: &Path) -> PathBuf {
    if paths.is_system() {
        PathBuf::from("/Library/LaunchDaemons").join(format!("{}.plist", label(paths)))
    } else {
        home.join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", label(paths)))
    }
}

pub fn unit_installed(paths: &RolePaths) -> bool {
    plist_path(paths).is_file()
}

pub fn log_hint(paths: &RolePaths) -> String {
    paths.state_dir.join("logs").display().to_string()
}

/// Render the launchd property list without writing anything.
pub fn render_plist(paths: &RolePaths) -> Result<String> {
    render_plist_with_home(paths, &home_dir())
}

fn render_plist_with_home(paths: &RolePaths, home: &Path) -> Result<String> {
    let service_home = if paths.is_system() {
        paths.state_dir.clone()
    } else {
        home.to_path_buf()
    };
    let logs = paths.state_dir.join("logs");
    let out_log = logs.join(format!("{}.out.log", paths.role.as_str()));
    let err_log = logs.join(format!("{}.err.log", paths.role.as_str()));
    let path_env = service_path(paths, &service_home);

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ");
    xml.push_str("\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n");
    xml.push_str("<plist version=\"1.0\">\n<dict>\n");
    push_key_string(&mut xml, "Label", &label(paths));
    // Associate the job with the app bundle so Login Items & Extensions shows
    // "Fleet" rather than the signing identity of the bare binary.
    xml.push_str("  <key>AssociatedBundleIdentifiers</key>\n  <array>\n");
    push_string(&mut xml, APP_BUNDLE_IDENTIFIER);
    xml.push_str("  </array>\n");
    xml.push_str("  <key>ProgramArguments</key>\n  <array>\n");
    for value in [
        paths.fleet_binary.display().to_string(),
        paths.role.as_str().to_string(),
        "run".to_string(),
        "--config".to_string(),
        paths.config.display().to_string(),
    ] {
        push_string(&mut xml, &value);
    }
    xml.push_str("  </array>\n");
    push_key_bool(&mut xml, "RunAtLoad", true);
    push_key_bool(&mut xml, "KeepAlive", true);
    push_key_integer(&mut xml, "ThrottleInterval", 2);
    push_key_string(&mut xml, "ProcessType", "Background");
    push_key_string(
        &mut xml,
        "WorkingDirectory",
        &paths.state_dir.display().to_string(),
    );
    push_key_string(&mut xml, "StandardOutPath", &out_log.display().to_string());
    push_key_string(
        &mut xml,
        "StandardErrorPath",
        &err_log.display().to_string(),
    );

    xml.push_str("  <key>EnvironmentVariables</key>\n  <dict>\n");
    push_dict_string(&mut xml, "HOME", &service_home.display().to_string());
    push_dict_string(&mut xml, "PATH", &path_env);
    xml.push_str("  </dict>\n");
    xml.push_str("</dict>\n</plist>\n");
    Ok(xml)
}

fn service_path(paths: &RolePaths, service_home: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut add = |value: &str| {
        if !value.is_empty() && !parts.iter().any(|part| part == value) {
            parts.push(value.to_string());
        }
    };

    if let Ok(current) = std::env::var("PATH") {
        for part in current.split(':') {
            add(part);
        }
    }
    if !paths.is_system() {
        add(&service_home
            .join(".local")
            .join("bin")
            .display()
            .to_string());
    }
    for part in DEFAULT_PATH.split(':') {
        add(part);
    }
    parts.join(":")
}

pub fn install(
    paths: &RolePaths,
    _admin_token: Option<&str>,
    start: bool,
    dry_run: bool,
    _yes: bool,
) -> Result<InstallOutcome> {
    let plist = render_plist(paths)?;
    let path = plist_path(paths);
    let domain = domain(paths);
    let service = format!("{domain}/{}", label(paths));
    let can_manage = !paths.is_system() || is_root();

    if dry_run {
        print_plan(paths, &path, &plist, start, can_manage);
        return Ok(InstallOutcome {
            unit_path: path,
            env_path: None,
            skipped: !can_manage,
        });
    }

    if !can_manage {
        let staging = std::env::temp_dir().join("fleet-launchd");
        fs::create_dir_all(&staging)
            .with_context(|| format!("failed to create {}", staging.display()))?;
        let staging_path = staging.join(format!("{}.plist", label(paths)));
        fs::write(&staging_path, &plist)
            .with_context(|| format!("failed to write {}", staging_path.display()))?;
        print_plan(paths, &path, &plist, start, false);
        println!();
        println!("staged plist: {}", staging_path.display());
        println!("run the commands above with sudo; nothing was installed");
        return Ok(InstallOutcome {
            unit_path: path,
            env_path: None,
            skipped: true,
        });
    }

    fs::create_dir_all(paths.state_dir.join("logs"))
        .with_context(|| "failed to create the service log directory")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, plist).with_context(|| format!("failed to write {}", path.display()))?;
    register_app_bundle();

    if start {
        let _ = Command::new("launchctl")
            .args(["bootout", &service])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        run_launchctl(&["bootstrap", &domain, &path.display().to_string()])?;
        println!("installed and started {}", path.display());
    } else {
        println!("installed {}", path.display());
        println!("start it with `fleet {} start`", paths.role.as_str());
    }

    Ok(InstallOutcome {
        unit_path: path,
        env_path: None,
        skipped: false,
    })
}

pub fn uninstall(paths: &RolePaths, dry_run: bool, yes: bool) -> Result<()> {
    let path = plist_path(paths);
    let domain = domain(paths);
    let service = format!("{domain}/{}", label(paths));
    let can_manage = !paths.is_system() || is_root();

    if dry_run {
        println!("launchctl bootout {service}");
        println!("rm -f {}", path.display());
        return Ok(());
    }
    if !can_manage {
        println!("run these commands with sudo:");
        println!("  launchctl bootout {service}");
        println!("  rm -f {}", path.display());
        return Ok(());
    }

    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = fs::remove_file(&path);
    let _ = yes;
    println!("removed {}", path.display());
    Ok(())
}

pub fn start(paths: &RolePaths) -> Result<()> {
    let path = plist_path(paths);
    if !path.is_file() {
        bail!(
            "{} is not installed; run `fleet {} install-service`",
            path.display(),
            paths.role.as_str()
        );
    }
    let domain = domain(paths);
    let service = format!("{domain}/{}", label(paths));
    register_app_bundle();
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    run_launchctl(&["bootstrap", &domain, &path.display().to_string()])
}

pub fn stop(paths: &RolePaths) -> Result<()> {
    let domain = domain(paths);
    let service = format!("{domain}/{}", label(paths));
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    println!("{} stopped", paths.role.as_str());
    Ok(())
}

pub fn restart(paths: &RolePaths) -> Result<()> {
    stop(paths)?;
    start(paths)
}

pub fn status(paths: &RolePaths) -> super::service::ServiceStatus {
    let domain = domain(paths);
    let service = format!("{domain}/{}", label(paths));
    let output = Command::new("launchctl").args(["print", &service]).output();
    let Ok(output) = output else {
        return super::service::ServiceStatus {
            backend: "launchd",
            active: false,
            state: "unknown".to_string(),
            detail: "launchctl print failed".to_string(),
        };
    };
    if !output.status.success() {
        return super::service::ServiceStatus {
            backend: "launchd",
            active: false,
            state: "not loaded".to_string(),
            detail: String::new(),
        };
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let state = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("state = "))
        .unwrap_or("unknown")
        .to_string();
    let active = state == "running";
    super::service::ServiceStatus {
        backend: "launchd",
        active,
        state,
        detail: format!("launchctl print {service}"),
    }
}

pub fn logs(paths: &RolePaths, lines: usize, follow: bool) -> Result<()> {
    let logs = paths.state_dir.join("logs");
    let out = logs.join(format!("{}.out.log", paths.role.as_str()));
    let err = logs.join(format!("{}.err.log", paths.role.as_str()));
    if !out.exists() && !err.exists() {
        bail!(
            "no launchd logs at {}; start the service first",
            logs.display()
        );
    }
    let mut command = Command::new("tail");
    command.arg("-n").arg(lines.to_string());
    if follow {
        command.arg("-f");
    }
    for path in [&out, &err] {
        if path.exists() {
            command.arg(path);
        }
    }
    let status = command.status().with_context(|| "failed to run tail")?;
    if !status.success() {
        bail!("tail exited with {status}");
    }
    Ok(())
}

fn print_plan(paths: &RolePaths, path: &Path, plist: &str, start: bool, can_manage: bool) {
    let domain = domain(paths);
    let service = format!("{domain}/{}", label(paths));
    if can_manage {
        println!("would write {}", path.display());
    } else {
        println!(
            "would stage a plist for manual installation at {}",
            path.display()
        );
    }
    println!();
    println!("{plist}");
    println!("launchctl bootout {service}");
    if can_manage {
        println!("launchctl bootstrap {domain} {}", path.display());
    } else {
        println!("sudo cp <staged plist> {}", path.display());
        println!("sudo chown root:wheel {}", path.display());
        println!("sudo chmod 644 {}", path.display());
        println!("sudo launchctl bootstrap {domain} {}", path.display());
    }
    if !start {
        println!("(service installation was requested without starting it)");
    }
    let _ = paths;
}

/// Register Fleet.app with LaunchServices so the Login Items UI can resolve
/// the bundle name for the job we are about to load. The release installer
/// does this too; source installs rely on this call.
fn register_app_bundle() {
    crate::macos_app::register_installed();
}

fn run_launchctl(args: &[&str]) -> Result<()> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| "failed to run launchctl")?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    bail!("launchctl {} failed: {}", args.join(" "), detail.trim())
}

fn domain(paths: &RolePaths) -> String {
    if paths.is_system() {
        "system".to_string()
    } else {
        format!("gui/{}", user_id())
    }
}

fn user_id() -> u32 {
    #[cfg(unix)]
    unsafe {
        libc::getuid()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn push_string(xml: &mut String, value: &str) {
    xml.push_str("    <string>");
    xml.push_str(&escape(value));
    xml.push_str("</string>\n");
}

fn push_key_string(xml: &mut String, key: &str, value: &str) {
    xml.push_str("  <key>");
    xml.push_str(&escape(key));
    xml.push_str("</key>\n");
    push_string(xml, value);
}

fn push_key_bool(xml: &mut String, key: &str, value: bool) {
    xml.push_str("  <key>");
    xml.push_str(&escape(key));
    xml.push_str("</key>\n");
    xml.push_str(if value { "  <true/>\n" } else { "  <false/>\n" });
}

fn push_key_integer(xml: &mut String, key: &str, value: i64) {
    xml.push_str("  <key>");
    xml.push_str(&escape(key));
    xml.push_str("</key>\n");
    xml.push_str(&format!("  <integer>{value}</integer>\n"));
}

fn push_dict_string(xml: &mut String, key: &str, value: &str) {
    xml.push_str("    <key>");
    xml.push_str(&escape(key));
    xml.push_str("</key>\n");
    xml.push_str("    <string>");
    xml.push_str(&escape(value));
    xml.push_str("</string>\n");
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
    use crate::config_paths::{Role, RolePathOverrides, Scope};

    fn paths(scope: Scope, home: &Path) -> RolePaths {
        let overrides = RolePathOverrides {
            bin: Some(std::env::current_exe().unwrap()),
            ..RolePathOverrides::default()
        };
        RolePaths::resolve_with(Role::Admin, scope, &overrides, "dev", "staff", home).unwrap()
    }

    #[test]
    fn plist_uses_user_launch_agents() {
        let home = Path::new("/Users/dev");
        let paths = paths(Scope::User, home);
        assert_eq!(
            plist_path_with_home(&paths, home),
            Path::new("/Users/dev/Library/LaunchAgents/dev.fleet.admin.plist")
        );
    }

    #[test]
    fn system_uses_launch_daemons() {
        let home = Path::new("/Users/dev");
        let paths = paths(Scope::System, home);
        assert_eq!(
            plist_path_with_home(&paths, home),
            Path::new("/Library/LaunchDaemons/dev.fleet.admin.plist")
        );
    }

    #[test]
    fn plist_contains_program_and_logs() {
        let home = Path::new("/Users/dev");
        let paths = paths(Scope::User, home);
        let plist = render_plist_with_home(&paths, home).unwrap();
        assert!(plist.contains("<string>dev.fleet.admin</string>"));
        assert!(plist.contains("<key>AssociatedBundleIdentifiers</key>"));
        assert!(plist.contains("<string>dev.fleet.app</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains(&paths.config.display().to_string()));
        assert!(plist.contains("KeepAlive"));
        assert!(plist.contains(".out.log"));
        assert!(plist.contains("<key>PATH</key>"));
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(escape("a&b<c>\"'"), "a&amp;b&lt;c&gt;&quot;&apos;");
    }
}

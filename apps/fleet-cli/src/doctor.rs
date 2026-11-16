//! Read-only `fleet doctor` diagnostics for both roles.

use std::path::Path;
use std::process::Command;

use anyhow::Result;
use serde_json::json;

use crate::config::expand_tilde;
use crate::config_paths::{discover_binary, RolePaths};
use crate::process::{check_database_separation, health, service};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Pass,
    Warn,
    Fail,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    pub message: String,
    pub fix: Option<String>,
}

impl Check {
    fn pass(message: impl Into<String>) -> Self {
        Self {
            level: Level::Pass,
            message: message.into(),
            fix: None,
        }
    }

    fn warn(message: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            message: message.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(message: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            message: message.into(),
            fix: Some(fix.into()),
        }
    }
}

pub async fn run(admin: &RolePaths, daemon: &RolePaths, as_json: bool) -> Result<i32> {
    let mut checks = Vec::new();
    binary_checks(admin, &mut checks);
    binary_checks(daemon, &mut checks);

    let admin_config = load_admin(admin, &mut checks);
    let daemon_config = load_daemon(daemon, &mut checks);

    if admin_config.is_some() && daemon_config.is_some() {
        if check_database_separation(admin).is_ok() {
            checks.push(Check::pass("admin and daemon databases are distinct"));
        } else {
            checks.push(Check::fail(
                "admin and daemon databases resolve to the same file",
                "edit [storage] database in the admin or daemon config",
            ));
        }
    }

    if let Some(config) = &admin_config {
        let token = config.auth.admin_token.trim();
        if !token.is_empty() && token.starts_with("fla_") {
            checks.push(Check::pass("admin token present (fla_)"));
        } else {
            checks.push(Check::fail(
                "admin token is missing or does not start with fla_",
                "fleet admin setup --rotate-admin-token",
            ));
        }
        ttl_checks(&config.auth, &mut checks);
        if let Some(static_dir) = &config.web.static_dir {
            if static_dir.is_dir() {
                checks.push(Check::pass("admin web assets directory exists"));
            } else {
                checks.push(Check::warn(
                    format!(
                        "admin web assets directory missing: {}",
                        static_dir.display()
                    ),
                    "run `pnpm build:web` or `fleet admin setup --web-dir <path>`",
                ));
            }
        }
        permission_check(&admin.config, "admin config", false, &mut checks);
    }

    if let Some(config) = &daemon_config {
        permission_check(&daemon.config, "daemon config", false, &mut checks);
        let server_url = config.connection.server_url.as_deref();
        match server_url {
            Some(url) if url.starts_with("ws://") || url.starts_with("wss://") => {
                checks.push(Check::pass("daemon server_url is a WebSocket URL"));
            }
            Some(url) => checks.push(Check::fail(
                format!("daemon server_url is not ws:// or wss://: {url}"),
                "fleet daemon setup --server ws://host:8787/ws/machine",
            )),
            None => checks.push(Check::warn(
                "daemon server_url is not configured",
                "fleet daemon setup --server ws://host:8787/ws/machine",
            )),
        }
        let key_file = expand_tilde(config.machine.key_file.clone());
        let token_file = fleet_core_auth::machine_token_path(&key_file);
        if token_file.is_file() {
            checks.push(Check::pass("daemon is paired (machine token present)"));
            permission_check(&token_file, "machine token", true, &mut checks);
        } else {
            checks.push(Check::warn(
                "daemon is not paired (machine token missing)",
                "fleet daemon setup --code <pairing-code>",
            ));
        }
    }

    health_checks(admin, daemon, &mut checks).await;
    conflict_checks(admin, daemon, &mut checks);
    scope_checks(admin, &mut checks);
    scope_checks(daemon, &mut checks);
    env_checks(&mut checks);

    let failed = checks.iter().any(|check| check.level == Level::Fail);
    if as_json {
        let checks: Vec<_> = checks
            .iter()
            .map(|check| {
                json!({
                    "level": check.level.as_str(),
                    "message": check.message,
                    "fix": check.fix,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "ok": !failed, "checks": checks }))?
        );
    } else {
        for check in &checks {
            println!("{:<5} {}", check.level.label(), check.message);
            if let Some(fix) = &check.fix {
                println!("      fix: {fix}");
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn binary_checks(paths: &RolePaths, checks: &mut Vec<Check>) {
    let version = env!("CARGO_PKG_VERSION");
    let role = paths.role;
    match discover_binary(role, None) {
        Ok(binary) => match Command::new(&binary).arg("--version").output() {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                if text.contains(version) {
                    checks.push(Check::pass(format!(
                        "{} version matches {version}",
                        role.binary_name()
                    )));
                } else {
                    checks.push(Check::warn(
                        format!(
                            "{} version does not report {version}: {}",
                            role.binary_name(),
                            text.trim()
                        ),
                        "rebuild or reinstall the server binaries",
                    ));
                }
            }
            _ => checks.push(Check::warn(
                format!("could not run {} --version", binary.display()),
                "rebuild or reinstall the server binaries",
            )),
        },
        Err(error) => checks.push(Check::fail(
            format!("{} not found: {error}", role.binary_name()),
            "install the server binaries or pass --bin",
        )),
    }
}

fn load_admin(
    paths: &RolePaths,
    checks: &mut Vec<Check>,
) -> Option<fleet_core_config::AdminConfig> {
    if !paths.config.is_file() {
        checks.push(Check::warn(
            format!("admin config missing: {}", paths.config.display()),
            "fleet admin setup",
        ));
        return None;
    }
    match fleet_core_config::AdminConfig::load(Some(&paths.config)) {
        Ok(config) => {
            checks.push(Check::pass("admin config parses"));
            Some(config)
        }
        Err(error) => {
            checks.push(Check::fail(
                format!("admin config is invalid: {error}"),
                "fix the TOML or rerun `fleet admin setup`",
            ));
            None
        }
    }
}

fn load_daemon(
    paths: &RolePaths,
    checks: &mut Vec<Check>,
) -> Option<fleet_core_config::DaemonConfig> {
    if !paths.config.is_file() {
        checks.push(Check::warn(
            format!("daemon config missing: {}", paths.config.display()),
            "fleet daemon setup",
        ));
        return None;
    }
    match fleet_core_config::DaemonConfig::load(Some(&paths.config)) {
        Ok(config) => {
            checks.push(Check::pass("daemon config parses"));
            Some(config)
        }
        Err(error) => {
            checks.push(Check::fail(
                format!("daemon config is invalid: {error}"),
                "fix the TOML or rerun `fleet daemon setup`",
            ));
            None
        }
    }
}

fn ttl_checks(auth: &fleet_core_config::AuthConfig, checks: &mut Vec<Check>) {
    let ttl = auth.pair_code_ttl_secs;
    let attempts = auth.pair_code_max_attempts;
    if ttl == 0 || attempts == 0 {
        checks.push(Check::fail(
            "pairing code TTL or attempt limit is zero",
            "set auth.pair_code_ttl_secs and auth.pair_code_max_attempts above zero",
        ));
    } else {
        checks.push(Check::pass("pairing code TTL and attempt limit are sane"));
    }
}

fn permission_check(path: &Path, label: &str, private: bool, checks: &mut Vec<Check>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(metadata) => {
                let mode = metadata.permissions().mode() & 0o777;
                if mode & 0o007 != 0 {
                    checks.push(Check::fail(
                        format!("{label} is world-accessible: {}", path.display()),
                        format!("chmod 0600 {}", path.display()),
                    ));
                } else if private && mode & 0o077 != 0 {
                    checks.push(Check::fail(
                        format!("{label} is readable by group/others: {}", path.display()),
                        format!("chmod 0600 {}", path.display()),
                    ));
                } else {
                    checks.push(Check::pass(format!("{label} permissions are safe")));
                }
            }
            Err(error) => checks.push(Check::warn(
                format!("could not inspect {label}: {error}"),
                format!("check that {} exists", path.display()),
            )),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, label, private, checks);
    }
}

async fn health_checks(admin: &RolePaths, daemon: &RolePaths, checks: &mut Vec<Check>) {
    for (label, paths) in [("admin", admin), ("daemon", daemon)] {
        match health::health_url(paths) {
            Ok(url) => {
                if health::probe_json(&url).await.is_some() {
                    checks.push(Check::pass(format!("{label} health ok at {url}")));
                } else {
                    checks.push(Check::warn(
                        format!("{label} health unavailable at {url}"),
                        format!("fleet {label} start"),
                    ));
                }
            }
            Err(error) => checks.push(Check::fail(
                format!("could not build the {label} health URL: {error}"),
                format!("fix the {label} config"),
            )),
        }
    }
}

fn conflict_checks(admin: &RolePaths, daemon: &RolePaths, checks: &mut Vec<Check>) {
    let before = checks.len();
    for paths in [admin, daemon] {
        if crate::process::supervisor_live(paths)
            && service::installed(paths)
            && service::status(paths).is_some_and(|status| status.active)
        {
            checks.push(Check::fail(
                format!(
                    "{} {} service and supervisor are both active",
                    paths.role.as_str(),
                    service::prompt_label_for(paths)
                ),
                format!("fleet {} stop --mode supervisor", paths.role.as_str()),
            ));
        }
    }
    if checks.len() == before {
        checks.push(Check::pass("service/supervisor conflict check complete"));
    }
}

fn scope_checks(paths: &RolePaths, checks: &mut Vec<Check>) {
    let system_path = crate::config_paths::is_system_config_path(&paths.config);
    let mixed = match paths.scope {
        crate::config_paths::Scope::User => system_path,
        crate::config_paths::Scope::System => !system_path,
    };
    if mixed {
        checks.push(Check::warn(
            format!(
                "{} install scope does not match config path {}",
                paths.role.as_str(),
                paths.config.display()
            ),
            format!(
                "pass --user or --system consistently for {}",
                paths.role.as_str()
            ),
        ));
    }
    if paths.is_system() && !service::installed(paths) {
        checks.push(Check::warn(
            format!(
                "{} {} service is not installed",
                paths.role.as_str(),
                service::prompt_label_for(paths)
            ),
            format!("sudo fleet {} install-service", paths.role.as_str()),
        ));
    }
}

fn env_checks(checks: &mut Vec<Check>) {
    const VARS: &[&str] = &[
        "FLEET_CONFIG",
        "FLEET_DB",
        "FLEET_BIND",
        "FLEET_ADMIN_URL",
        "FLEET_ADMIN_TOKEN",
        "FLEET_SERVER_URL",
        "FLEET_DATA_DIR",
        "FLEET_MACHINE_NAME",
        "FLEET_PAIR_CODE",
    ];
    let set: Vec<&str> = VARS
        .iter()
        .copied()
        .filter(|key| std::env::var_os(key).is_some())
        .collect();
    if set.is_empty() {
        checks.push(Check::pass("no FLEET_* config overrides are set"));
    } else {
        checks.push(Check::warn(
            format!("FLEET_* overrides are set: {}", set.join(", ")),
            "unset them if the on-disk configuration should be authoritative",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_render_stable_labels() {
        assert_eq!(Level::Pass.label(), "PASS");
        assert_eq!(Level::Warn.label(), "WARN");
        assert_eq!(Level::Fail.label(), "FAIL");
        assert_eq!(Level::Fail.as_str(), "fail");
    }

    #[test]
    fn fail_and_warn_carry_fix_hints() {
        assert!(Check::warn("w", "wfix").fix.is_some());
        assert!(Check::fail("f", "ffix").fix.is_some());
        assert!(Check::pass("p").fix.is_none());
    }
}

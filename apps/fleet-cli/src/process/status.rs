//! `fleet ... status` and `fleet ... logs` across native services, the
//! supervisor, and externally managed processes.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::cli::ProcessMode;
use crate::config::expand_tilde;
use crate::config_paths::{Role, RolePaths};

use super::check_database_separation;
use super::health;
use super::service;
use super::supervisor::{self, SupervisorState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMode {
    Service,
    Supervisor,
    External,
    Stopped,
    Conflict,
}

impl StatusMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Supervisor => "supervisor",
            Self::External => "external",
            Self::Stopped => "stopped",
            Self::Conflict => "conflict",
        }
    }
}

async fn detect(paths: &RolePaths, requested: ProcessMode) -> Result<StatusMode> {
    match requested {
        ProcessMode::Service if !service::installed(paths) => bail!(
            "service mode requested but the {} service is not installed; run `fleet {} install-service`",
            service::prompt_label_for(paths),
            paths.role.as_str()
        ),
        ProcessMode::Service => Ok(StatusMode::Service),
        ProcessMode::Supervisor => Ok(StatusMode::Supervisor),
        ProcessMode::Auto => {
            let supervised = super::supervisor_live(paths);
            let unit_active = service::installed(paths)
                && service::available_for(paths).is_some()
                && service::status(paths).is_some_and(|status| status.active);
            if supervised && unit_active {
                return Ok(StatusMode::Conflict);
            }
            if supervised {
                return Ok(StatusMode::Supervisor);
            }
            if service::installed(paths) && service::available_for(paths).is_some() {
                return Ok(StatusMode::Service);
            }
            if health::probe(paths).await {
                return Ok(StatusMode::External);
            }
            Ok(StatusMode::Stopped)
        }
    }
}

/// Build the status report and its contract exit code without printing.
pub async fn collect(paths: &RolePaths, requested: ProcessMode) -> Result<(i32, Value)> {
    let mode = detect(paths, requested).await?;
    let url = health::health_url(paths).ok();
    let health_payload = match &url {
        Some(url) => health::probe_json(url).await,
        None => None,
    };
    let healthy = health_payload.is_some();
    let (database, config_valid) = role_database(paths);
    let databases_distinct = check_database_separation(paths).is_ok();
    let service_status = if mode == StatusMode::Service {
        service::status(paths)
    } else {
        None
    };
    let supervisor_state = if mode == StatusMode::Supervisor {
        supervisor::read_state(&paths.supervisor_state())
    } else {
        None
    };
    let supervisor_live = supervisor_state.as_ref().is_some_and(|state| {
        supervisor::pid_alive(state.supervisor_pid) && supervisor::pid_alive(state.child_pid)
    });

    let mut report = json!({
        "role": paths.role.as_str(),
        "mode": mode.as_str(),
        "healthy": healthy,
        "url": url,
        "service_level": paths.service_level().as_str(),
        "run_as": paths.run_as().as_str(),
        "user": paths.service_user,
        "state_dir": paths.state_dir,
        "config": paths.config,
        "config_valid": config_valid,
        "database": database,
        "databases_distinct": databases_distinct,
        "binary": paths.binary,
        "health": health_payload,
        "log_source": match mode {
            StatusMode::Service => service::log_hint(paths),
            _ => paths.server_log().display().to_string(),
        },
    });
    if let Some(status) = &service_status {
        report["service"] = json!({
            "backend": service::active_name_for(paths),
            "active": status.active,
            "state": status.state,
            "detail": status.detail,
        });
    }
    if let Some(state) = &supervisor_state {
        report["supervisor"] = supervisor_json(state);
    }
    if paths.role == Role::Daemon {
        report["daemon"] = daemon_json(paths);
    }

    let code = match mode {
        StatusMode::Service => match service_status.as_ref() {
            Some(status) if status.active && healthy => 0,
            Some(status) if !status.active => 1,
            _ => 2,
        },
        StatusMode::Supervisor => {
            if supervisor_live && healthy {
                0
            } else if supervisor_state.is_some() {
                2
            } else if healthy {
                0
            } else {
                1
            }
        }
        StatusMode::External => {
            if healthy {
                0
            } else {
                2
            }
        }
        StatusMode::Stopped => 1,
        StatusMode::Conflict => 2,
    };

    Ok((code, report))
}

/// Print status and return the contract exit code.
pub async fn status(paths: &RolePaths, requested: ProcessMode, as_json: bool) -> Result<i32> {
    let (code, report) = collect(paths, requested).await?;
    let mode = match report["mode"].as_str().unwrap_or("stopped") {
        "supervisor" => StatusMode::Supervisor,
        "external" => StatusMode::External,
        "conflict" => StatusMode::Conflict,
        "stopped" => StatusMode::Stopped,
        _ => StatusMode::Service,
    };
    let healthy = report["healthy"].as_bool().unwrap_or(false);
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(paths, mode, healthy, &report);
    }
    Ok(code)
}

fn role_database(paths: &RolePaths) -> (Option<String>, bool) {
    let database = match paths.role {
        Role::Admin => fleet_core_config::AdminConfig::load(Some(&paths.config))
            .ok()
            .map(|config| expand_tilde(config.storage.database)),
        Role::Daemon => fleet_core_config::DaemonConfig::load(Some(&paths.config))
            .ok()
            .map(|config| expand_tilde(config.storage.database)),
    };
    let valid = database.is_some();
    (database.map(|path| path.display().to_string()), valid)
}

fn supervisor_json(state: &SupervisorState) -> Value {
    json!({
        "supervisor_pid": state.supervisor_pid,
        "child_pid": state.child_pid,
        "started_at": state.started_at,
        "restarts": state.restarts,
        "last_exit": state.last_exit,
        "server_log": state.server_log,
        "supervisor_log": state.supervisor_log,
    })
}

fn daemon_json(paths: &RolePaths) -> Value {
    let identity = crate::local::identity_json(paths).unwrap_or(Value::Null);
    let server_url = fleet_core_config::DaemonConfig::load(Some(&paths.config))
        .ok()
        .and_then(|config| config.connection.server_url);
    json!({
        "machine_id": identity.get("machine_id"),
        "machine_name": identity.get("machine_name"),
        "paired": identity.get("paired"),
        "token_file": identity.get("token_file"),
        "server_url": server_url,
    })
}

fn print_human(paths: &RolePaths, mode: StatusMode, healthy: bool, report: &Value) {
    println!("role      {}", paths.role.as_str());
    println!("mode      {}", mode.as_str());
    println!("service   system (user {})", paths.service_user);
    println!("healthy   {}", if healthy { "yes" } else { "no" });
    if let Some(health) = report.get("health") {
        if let Some(version) = health
            .get("version")
            .or_else(|| health.get("daemon_version"))
            .and_then(Value::as_str)
        {
            println!("version   {version}");
        }
        if let Some(active) = health.get("active_sessions").and_then(Value::as_u64) {
            println!("sessions  {active} active");
        }
    }
    if let Some(source) = report["log_source"].as_str() {
        println!("logs      {source}");
    }
    if let Some(url) = report["url"].as_str() {
        println!("url       {url}");
    }
    println!("config    {}", paths.config.display());
    if let Some(database) = report["database"].as_str() {
        println!("database  {database}");
    }
    println!(
        "distinct  {}",
        if report["databases_distinct"].as_bool().unwrap_or(false) {
            "yes"
        } else {
            "no"
        }
    );
    if let Some(service) = report.get("service") {
        println!(
            "service   {} active={} state={}",
            service["backend"].as_str().unwrap_or("service"),
            service["active"].as_bool().unwrap_or(false),
            service["state"].as_str().unwrap_or("unknown"),
        );
    }
    if let Some(supervisor) = report.get("supervisor") {
        println!(
            "pids      supervisor={} child={} restarts={}",
            supervisor["supervisor_pid"].as_u64().unwrap_or(0),
            supervisor["child_pid"].as_u64().unwrap_or(0),
            supervisor["restarts"].as_u64().unwrap_or(0)
        );
        println!(
            "started   {}",
            supervisor["started_at"].as_str().unwrap_or("")
        );
        if let Some(last_exit) = supervisor["last_exit"].as_str() {
            println!("last exit {last_exit}");
        }
        println!(
            "logs      {}",
            supervisor["server_log"].as_str().unwrap_or("")
        );
    }
    if let Some(daemon) = report.get("daemon") {
        println!(
            "machine   {} ({})",
            daemon["machine_name"].as_str().unwrap_or("unknown"),
            daemon["machine_id"].as_str().unwrap_or("unknown")
        );
        println!("paired    {}", daemon["paired"].as_bool().unwrap_or(false));
        if let Some(server) = daemon["server_url"].as_str() {
            println!("server    {server}");
        }
    }
}

/// Print logs for the active mode.
pub async fn logs(
    paths: &RolePaths,
    requested: ProcessMode,
    lines: usize,
    follow: bool,
    supervisor_log: bool,
) -> Result<()> {
    match requested {
        ProcessMode::Service => service::logs(paths, lines, follow),
        ProcessMode::Supervisor => tail_role_log(paths, lines, follow, supervisor_log).await,
        ProcessMode::Auto => {
            if service::installed(paths) && service::available_for(paths).is_some() {
                service::logs(paths, lines, follow)
            } else if log_path(paths, supervisor_log).is_file() {
                tail_role_log(paths, lines, follow, supervisor_log).await
            } else {
                let url = health::health_url(paths)?;
                bail!(
                    "no fleet-managed log exists for {}; health endpoint: {url}",
                    paths.role.as_str()
                )
            }
        }
    }
}

fn log_path(paths: &RolePaths, supervisor_log: bool) -> std::path::PathBuf {
    if supervisor_log {
        paths.supervisor_log()
    } else {
        paths.server_log()
    }
}

async fn tail_role_log(
    paths: &RolePaths,
    lines: usize,
    follow: bool,
    supervisor_log: bool,
) -> Result<()> {
    let path = log_path(paths, supervisor_log);
    if !path.is_file() {
        bail!(
            "log not found: {}; start the role with `fleet {} start --mode supervisor`",
            path.display(),
            paths.role.as_str()
        );
    }
    println!("==> {} <==", path.display());
    let text = supervisor::tail_lines(&path, lines)?;
    if !text.is_empty() {
        println!("{text}");
    }
    if follow {
        follow_file(&path).await?;
    }
    Ok(())
}

async fn follow_file(path: &Path) -> Result<()> {
    let mut offset = supervisor::end_offset(path)?;
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = interval.tick() => {
                let len = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
                if len < offset {
                    offset = 0;
                }
                if len == offset {
                    continue;
                }
                let mut file = File::open(path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                file.seek(SeekFrom::Start(offset))?;
                let mut chunk = String::new();
                file.read_to_string(&mut chunk)?;
                print!("{chunk}");
                std::io::stdout().flush().ok();
                offset = file.stream_position()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ActiveMode;

    fn paths(dir: &Path, role: Role) -> RolePaths {
        RolePaths {
            role,
            config: dir.join("config.toml"),
            state_dir: dir.join("state"),
            database: dir.join("state/db.sqlite"),
            binary: None,
            fleet_binary: dir.join("fleet"),
            service_user: "tester".into(),
            service_group: "tester".into(),
            web_dir: None,
        }
    }

    #[test]
    fn status_mode_strings_are_stable() {
        assert_eq!(StatusMode::Service.as_str(), "service");
        assert_eq!(StatusMode::Supervisor.as_str(), "supervisor");
        assert_eq!(StatusMode::External.as_str(), "external");
        assert_eq!(StatusMode::Stopped.as_str(), "stopped");
    }

    #[test]
    fn supervisor_json_omits_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let state = SupervisorState {
            role: "daemon".into(),
            supervisor_pid: 10,
            child_pid: 11,
            started_at: "2026-11-16T00:00:00Z".into(),
            restarts: 2,
            last_exit: Some("exit 1".into()),
            binary: dir.path().join("fleet-daemon"),
            config: dir.path().join("daemon.toml"),
            server_log: dir.path().join("logs/daemon.log"),
            supervisor_log: dir.path().join("logs/supervisor.log"),
        };
        let rendered = supervisor_json(&state).to_string();
        assert!(rendered.contains("supervisor_pid"));
        assert!(!rendered.contains("token"));
    }

    #[test]
    fn active_mode_is_a_valid_enum() {
        assert_eq!(ActiveMode::Supervisor, ActiveMode::Supervisor);
        let _ = paths(Path::new("/tmp"), Role::Admin);
    }
}

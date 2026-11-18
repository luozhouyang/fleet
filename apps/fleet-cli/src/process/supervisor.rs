//! Detached fallback supervisor for hosts without a native service manager.
//!
//! `fleet admin|daemon start --mode supervisor` spawns
//! `fleet __supervise <role>`, which owns the server process, writes
//! `run/supervisor.json`, restarts crashes with backoff, and rotates logs.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command as TokioCommand};
use tokio::time::{sleep, timeout};

use crate::config::{create_private_dir, write_private};
use crate::config_paths::{Role, RolePathOverrides, RolePaths};

use super::check_database_separation;
use super::health;
use super::service;

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
const BACKOFF_SECONDS: [u64; 6] = [1, 2, 4, 8, 16, 30];
const RESTART_RESET: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

// `SIGKILL` does not exist in the Windows CRT. The supervisor's signal
// helpers are no-ops on non-Unix platforms, so the value is never used there.
#[cfg(unix)]
const FORCE_KILL: libc::c_int = libc::SIGKILL;
#[cfg(not(unix))]
const FORCE_KILL: libc::c_int = 0;

/// Atomically written supervisor state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorState {
    pub role: String,
    pub supervisor_pid: u32,
    pub child_pid: u32,
    pub started_at: String,
    pub restarts: u32,
    pub last_exit: Option<String>,
    pub binary: PathBuf,
    pub config: PathBuf,
    pub server_log: PathBuf,
    pub supervisor_log: PathBuf,
}

/// Read the state file. Missing or malformed state is treated as absent.
pub fn read_state(path: &Path) -> Option<SupervisorState> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Write state through a temporary file and rename it into place.
pub fn write_state(path: &Path, state: &SupervisorState) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("state path has no parent: {}", path.display()))?;
    create_private_dir(parent)?;
    let temp = parent.join(format!(".supervisor.json.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(state).context("failed to serialize supervisor state")?;
    write_private(&temp, &bytes)?;
    fs::rename(&temp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

/// Exclusive lock held while starting or stopping the supervisor.
pub struct StartLock {
    file: File,
}

impl StartLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            use std::os::unix::io::AsRawFd;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc != 0 {
                bail!(
                    "failed to lock {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                );
            }
        }
        Ok(Self { file })
    }
}

impl Drop for StartLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as i32, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// True when `/proc/<pid>/cmdline` contains every needle. Other platforms can
/// only check liveness.
pub fn pid_matches(pid: u32, needles: &[&str]) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
            return false;
        };
        let command = String::from_utf8_lossy(&raw).replace('\0', " ");
        needles.iter().all(|needle| command.contains(needle))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = needles;
        pid_alive(pid)
    }
}

fn send_signal(pid: u32, signal: i32) -> Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as i32, signal) };
        if rc != 0 {
            bail!(
                "failed to signal pid {pid}: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
        bail!("process signals are not supported on this platform")
    }
}

fn send_group_signal(pid: u32, signal: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
    }
}

/// Copy-and-truncate rotation, keeping one `.1` file.
pub fn rotate_if_needed(path: &Path, max_bytes: u64) -> Result<bool> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(false);
    };
    if metadata.len() <= max_bytes {
        return Ok(false);
    }
    let rotated = PathBuf::from(format!("{}.1", path.display()));
    fs::copy(path, &rotated).with_context(|| format!("failed to rotate {}", path.display()))?;
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("failed to truncate {}", path.display()))?;
    Ok(true)
}

/// Tail the last `lines` lines without loading the file twice.
pub fn tail_lines(path: &Path, lines: usize) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    Ok(all[start..].join("\n"))
}

/// Start the role under the detached supervisor.
pub async fn start(paths: &RolePaths, wait_limit: Duration) -> Result<()> {
    let _lock = StartLock::acquire(&paths.start_lock())?;
    let url = health::health_url(paths)?;
    if health::probe_url(&url).await {
        println!("{} is already running at {url}", paths.role.as_str());
        return Ok(());
    }
    if let Some(state) = read_state(&paths.supervisor_state()) {
        if pid_alive(state.supervisor_pid)
            && pid_matches(state.supervisor_pid, &["__supervise", paths.role.as_str()])
        {
            println!(
                "{} is already running under the fallback supervisor (pid {}); health is not ready yet",
                paths.role.as_str(),
                state.supervisor_pid
            );
            return Ok(());
        }
    }
    if service::installed(paths)
        && service::config_matches(paths)
        && service::status(paths).is_some_and(|status| status.active)
    {
        bail!(
            "{} is already running under the {} service; use `fleet {} stop` or pass `--mode service`",
            paths.role.as_str(),
            service::prompt_label_for(paths),
            paths.role.as_str()
        );
    }

    validate(paths)?;
    create_private_dir(&paths.run_dir())?;
    create_private_dir(&paths.logs_dir())?;
    let paths = canonicalize(paths);
    let _ = rotate_if_needed(&paths.server_log(), MAX_LOG_BYTES)?;
    let _ = rotate_if_needed(&paths.supervisor_log(), MAX_LOG_BYTES)?;

    let supervisor_pid = spawn_supervisor(&paths)?;
    if wait_for_ready(&paths, &url, wait_limit).await {
        println!(
            "{} started (supervisor pid {supervisor_pid}, mode supervisor)",
            paths.role.as_str()
        );
        println!("config: {}", paths.config.display());
        println!("health: {url}");
        println!("logs:   {}", paths.server_log().display());
        Ok(())
    } else {
        let _ = send_signal(supervisor_pid, libc::SIGTERM);
        let _ = wait_for_pid_exit(supervisor_pid, Duration::from_secs(5)).await;
        let _ = fs::remove_file(paths.supervisor_state());
        let detail = tail_lines(&paths.supervisor_log(), 20).unwrap_or_default();
        bail!(
            "{} did not become healthy within {}s; supervisor log: {}\n{}",
            paths.role.as_str(),
            wait_limit.as_secs(),
            paths.supervisor_log().display(),
            detail
        )
    }
}

/// Make paths independent of the caller's working directory before `setsid`.
fn canonicalize(paths: &RolePaths) -> RolePaths {
    let mut paths = paths.clone();
    paths.config = fs::canonicalize(&paths.config).unwrap_or_else(|_| paths.config.clone());
    paths.state_dir =
        fs::canonicalize(&paths.state_dir).unwrap_or_else(|_| paths.state_dir.clone());
    if let Some(binary) = &paths.binary {
        paths.binary = Some(fs::canonicalize(binary).unwrap_or_else(|_| binary.clone()));
    }
    paths
}

fn validate(paths: &RolePaths) -> Result<()> {
    if !paths.config.is_file() {
        bail!(
            "{} config not found: {}; run `fleet {} setup` first",
            paths.role.as_str(),
            paths.config.display(),
            paths.role.as_str()
        );
    }
    check_database_separation(paths)?;
    match paths.role {
        Role::Admin => {
            let config = fleet_core_config::AdminConfig::load(Some(&paths.config))
                .with_context(|| format!("invalid admin config {}", paths.config.display()))?;
            if config.auth.admin_token.trim().is_empty() {
                bail!(
                    "admin token is empty; run `fleet admin setup` to generate one before starting"
                );
            }
        }
        Role::Daemon => {
            fleet_core_config::DaemonConfig::load(Some(&paths.config))
                .with_context(|| format!("invalid daemon config {}", paths.config.display()))?;
        }
    }
    paths.require_binary()?;
    Ok(())
}

fn spawn_supervisor(paths: &RolePaths) -> Result<u32> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.supervisor_log())
        .with_context(|| format!("failed to open {}", paths.supervisor_log().display()))?;
    let mut command = Command::new(&paths.fleet_binary);
    command
        .arg("__supervise")
        .arg(paths.role.as_str())
        .arg("--config")
        .arg(&paths.config)
        .arg("--state-dir")
        .arg(&paths.state_dir);
    if let Some(binary) = &paths.binary {
        command.arg("--bin").arg(binary);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn {}", paths.fleet_binary.display()))?;
    Ok(child.id())
}

async fn wait_for_ready(paths: &RolePaths, url: &str, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        let state = read_state(&paths.supervisor_state());
        if state.is_some_and(|state| pid_alive(state.child_pid) && pid_alive(state.supervisor_pid))
            && health::probe_url(url).await
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(200)).await;
    }
}

/// Stop the fallback supervisor and its server.
pub async fn stop(paths: &RolePaths, wait_limit: Duration) -> Result<()> {
    let _lock = StartLock::acquire(&paths.start_lock())?;
    let Some(state) = read_state(&paths.supervisor_state()) else {
        if health::probe(paths).await {
            bail!(
                "{} is running but was not started by fleet (no supervisor state); stop it manually or use the native service",
                paths.role.as_str()
            );
        }
        println!("{} is not running", paths.role.as_str());
        return Ok(());
    };

    if !pid_alive(state.supervisor_pid)
        || !pid_matches(state.supervisor_pid, &["__supervise", paths.role.as_str()])
    {
        cleanup_stale(paths, &state)?;
        println!(
            "{} is not running (removed stale state)",
            paths.role.as_str()
        );
        return Ok(());
    }

    send_signal(state.supervisor_pid, libc::SIGTERM)?;
    if !wait_for_pid_exit(state.supervisor_pid, wait_limit).await {
        let _ = send_signal(state.supervisor_pid, FORCE_KILL);
        let _ = wait_for_pid_exit(state.supervisor_pid, Duration::from_secs(2)).await;
    }
    if pid_alive(state.child_pid) && pid_matches(state.child_pid, &[paths.role.binary_name()]) {
        send_group_signal(state.child_pid, libc::SIGTERM);
        if !wait_for_pid_exit(state.child_pid, Duration::from_secs(5)).await {
            send_group_signal(state.child_pid, FORCE_KILL);
        }
    }
    let _ = fs::remove_file(paths.supervisor_state());
    println!("{} stopped", paths.role.as_str());
    Ok(())
}

fn cleanup_stale(paths: &RolePaths, state: &SupervisorState) -> Result<()> {
    if pid_alive(state.child_pid) && pid_matches(state.child_pid, &[paths.role.binary_name()]) {
        send_group_signal(state.child_pid, libc::SIGTERM);
    }
    let _ = fs::remove_file(paths.supervisor_state());
    Ok(())
}

async fn wait_for_pid_exit(pid: u32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        sleep(Duration::from_millis(100)).await;
    }
    !pid_alive(pid)
}

/// Hidden `fleet __supervise <role>` entry point.
pub async fn supervise(
    role: Role,
    config: PathBuf,
    state_dir: PathBuf,
    binary: PathBuf,
) -> Result<()> {
    let overrides = RolePathOverrides {
        config: Some(config),
        state_dir: Some(state_dir),
        database: None,
        bin: Some(binary.clone()),
    };
    let mut paths = RolePaths::resolve_config(role, &overrides)?;
    paths.binary = Some(binary);
    check_database_separation(&paths)?;
    create_private_dir(&paths.run_dir())?;
    create_private_dir(&paths.logs_dir())?;

    let mut state = SupervisorState {
        role: paths.role.as_str().to_string(),
        supervisor_pid: std::process::id(),
        child_pid: 0,
        started_at: chrono::Utc::now().to_rfc3339(),
        restarts: 0,
        last_exit: None,
        binary: paths.require_binary()?.to_path_buf(),
        config: paths.config.clone(),
        server_log: paths.server_log(),
        supervisor_log: paths.supervisor_log(),
    };
    write_state(&paths.supervisor_state(), &state)?;
    let result = run_loop(&paths, &mut state).await;
    let _ = fs::remove_file(paths.supervisor_state());
    result
}

#[cfg(unix)]
async fn run_loop(paths: &RolePaths, state: &mut SupervisorState) -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = signal(SignalKind::terminate()).context("failed to listen for SIGTERM")?;
    let mut interrupt = signal(SignalKind::interrupt()).context("failed to listen for SIGINT")?;
    let mut hangup = signal(SignalKind::hangup()).context("failed to listen for SIGHUP")?;
    let mut rotate = tokio::time::interval(Duration::from_secs(60));
    rotate.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    rotate.tick().await;

    let mut backoff = 0usize;
    loop {
        let mut child = spawn_child(paths)?;
        state.child_pid = child.id().unwrap_or(0);
        write_state(&paths.supervisor_state(), state)?;
        let started = Instant::now();

        loop {
            tokio::select! {
                status = child.wait() => {
                    let status = status.context("failed to wait for server")?;
                    state.last_exit = Some(exit_text(status));
                    state.restarts = state.restarts.saturating_add(1);
                    write_state(&paths.supervisor_state(), state)?;
                    if started.elapsed() >= RESTART_RESET {
                        backoff = 0;
                    }
                    break;
                }
                _ = terminate.recv() => {
                    terminate_child(&mut child).await;
                    return Ok(());
                }
                _ = interrupt.recv() => {
                    terminate_child(&mut child).await;
                    return Ok(());
                }
                _ = hangup.recv() => {
                    // Use `fleet ... restart` for a config reload.
                }
                _ = rotate.tick() => {
                    let _ = rotate_if_needed(&paths.server_log(), MAX_LOG_BYTES);
                    let _ = rotate_if_needed(&paths.supervisor_log(), MAX_LOG_BYTES);
                }
            }
        }

        let delay = BACKOFF_SECONDS[backoff.min(BACKOFF_SECONDS.len() - 1)];
        backoff = (backoff + 1).min(BACKOFF_SECONDS.len() - 1);
        tokio::select! {
            _ = sleep(Duration::from_secs(delay)) => {}
            _ = terminate.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
            _ = hangup.recv() => {}
        }
    }
}

#[cfg(not(unix))]
async fn run_loop(_paths: &RolePaths, _state: &mut SupervisorState) -> Result<()> {
    bail!("the detached supervisor is not supported on this platform");
}

fn spawn_child(paths: &RolePaths) -> Result<Child> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.server_log())
        .with_context(|| format!("failed to open {}", paths.server_log().display()))?;
    let binary = paths.require_binary()?;
    let mut command = TokioCommand::new(binary);
    command
        .arg("--config")
        .arg(&paths.config)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
        unsafe {
            command.as_std_mut().pre_exec(|| {
                #[cfg(target_os = "linux")]
                {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                }
                Ok(())
            });
        }
    }
    command
        .spawn()
        .with_context(|| format!("failed to spawn {}", binary.display()))
}

async fn terminate_child(child: &mut Child) {
    if let Some(pid) = child.id() {
        send_group_signal(pid, libc::SIGTERM);
    }
    if timeout(STOP_TIMEOUT, child.wait()).await.is_err() {
        if let Some(pid) = child.id() {
            send_group_signal(pid, FORCE_KILL);
        }
        let _ = child.wait().await;
    }
}

fn exit_text(status: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "unknown exit".to_string(),
    }
}

/// Seek to the end of `path`; used by `logs --follow`.
pub fn end_offset(path: &Path) -> Result<u64> {
    let mut file = File::open(path)?;
    Ok(file.seek(SeekFrom::End(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(dir: &Path) -> SupervisorState {
        SupervisorState {
            role: "admin".into(),
            supervisor_pid: 1,
            child_pid: 2,
            started_at: "2026-11-16T00:00:00Z".into(),
            restarts: 0,
            last_exit: None,
            binary: dir.join("fleet-admin"),
            config: dir.join("admin.toml"),
            server_log: dir.join("logs/admin.log"),
            supervisor_log: dir.join("logs/supervisor.log"),
        }
    }

    #[test]
    fn state_round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run/supervisor.json");
        let value = state(dir.path());
        write_state(&path, &value).unwrap();

        assert_eq!(read_state(&path).unwrap(), value);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"role\": \"admin\""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn malformed_state_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.json");
        fs::write(&path, "not json").unwrap();
        assert!(read_state(&path).is_none());
    }

    #[test]
    fn log_rotation_keeps_one_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        fs::write(&path, "0123456789abcdef").unwrap();

        assert!(rotate_if_needed(&path, 8).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert_eq!(
            fs::read_to_string(format!("{}.1", path.display())).unwrap(),
            "0123456789abcdef"
        );
        assert!(!rotate_if_needed(&path, 8).unwrap());
    }

    #[test]
    fn tail_lines_returns_the_requested_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(tail_lines(&path, 2).unwrap(), "c\nd");
        assert_eq!(tail_lines(&path, 99).unwrap(), "a\nb\nc\nd");
    }

    #[test]
    fn pid_matching_uses_the_command_line() {
        let pid = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().to_string();
        assert!(pid_alive(pid));
        assert!(pid_matches(pid, &[&name]));
        // Only Linux can inspect /proc/<pid>/cmdline; elsewhere liveness is the
        // best available answer.
        #[cfg(target_os = "linux")]
        assert!(!pid_matches(pid, &["definitely-not-this-process"]));
        #[cfg(not(target_os = "linux"))]
        assert!(pid_matches(pid, &["definitely-not-this-process"]));
        assert!(!pid_alive(0));
    }

    #[test]
    fn exit_text_handles_codes() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(
                exit_text(std::process::ExitStatus::from_raw(3 << 8)),
                "exit 3"
            );
        }
    }
}

//! ACP v1 terminal client.
//!
//! v1 agents may ask the client to run a command and stream its output. Fleet
//! executes the argv directly (never through a shell), confines the working
//! directory to the session workspace, bounds retained output and caps the
//! number of live terminals per session. The agent process itself is not
//! constrained by this module; the ACP permission flow remains the authority
//! for approving the command.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::v1::schema as v1s;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use super::session::AcpSessionHandle;

/// Default retained output per terminal.
pub const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
/// Hard upper bound for retained output per terminal.
pub const MAX_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
/// Lower bound so a tiny requested limit cannot discard the exit context.
pub const MIN_OUTPUT_LIMIT: usize = 4 * 1024;
/// Maximum live terminals per session.
pub const MAX_TERMINALS: usize = 16;
/// How long a `wait_for_exit` handler waits before reporting a timeout.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(600);
/// Upper bound on how long the exit watcher waits for the pipe readers after
/// the child exits. Readers normally finish immediately; a descendant that
/// keeps a pipe open must not delay exit reporting indefinitely.
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Live terminals for one ACP session.
#[derive(Default)]
pub struct TerminalManager {
    slots: Mutex<TerminalSlots>,
}

#[derive(Default)]
struct TerminalSlots {
    terminals: HashMap<String, Arc<Terminal>>,
    /// Slots reserved by `create` calls that are still starting their
    /// process. Counting reservations under the same lock as the live map
    /// closes the check-then-insert race between concurrent creates.
    reserved: usize,
    /// Incremented by `clear` so a create that finishes after a session
    /// shutdown cannot resurrect a terminal.
    generation: u64,
}

/// RAII reservation of one terminal slot.
struct SlotReservation<'a> {
    manager: &'a TerminalManager,
    generation: u64,
    active: bool,
}

impl<'a> SlotReservation<'a> {
    fn acquire(manager: &'a TerminalManager) -> Result<Self, String> {
        let mut slots = manager.slots.lock();
        if slots.terminals.len() + slots.reserved >= MAX_TERMINALS {
            return Err(format!(
                "session already has {MAX_TERMINALS} live terminals; release one first"
            ));
        }
        slots.reserved += 1;
        Ok(Self {
            manager,
            generation: slots.generation,
            active: true,
        })
    }

    /// Turn the reservation into a live terminal.
    fn commit(mut self, terminal_id: String, terminal: Arc<Terminal>) {
        let mut slots = self.manager.slots.lock();
        slots.reserved = slots.reserved.saturating_sub(1);
        if slots.generation == self.generation {
            slots.terminals.insert(terminal_id, terminal);
        } else {
            // The session was cleared while the process was starting.
            let _ = terminate(&mut terminal.child.lock());
        }
        self.active = false;
    }
}

impl Drop for SlotReservation<'_> {
    fn drop(&mut self) {
        if self.active {
            let mut slots = self.manager.slots.lock();
            slots.reserved = slots.reserved.saturating_sub(1);
        }
    }
}

impl TerminalManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a command for the agent.
    pub async fn create(
        &self,
        session: &AcpSessionHandle,
        request: &v1s::CreateTerminalRequest,
    ) -> Result<v1s::CreateTerminalResponse, String> {
        if request.command.trim().is_empty() {
            return Err("terminal command must not be empty".to_string());
        }
        // Reserve the slot before the first await; every error path below
        // releases it through the guard's drop.
        let reservation = SlotReservation::acquire(self)?;
        let root = session.workspace_root.lock().clone();
        let cwd = match &request.cwd {
            Some(cwd) => cwd.to_string_lossy().to_string(),
            None => session.session_cwd.lock().clone(),
        };
        let cwd = resolve_terminal_cwd(&root, &cwd).await?;

        let limit = request
            .output_byte_limit
            .map(|limit| limit as usize)
            .unwrap_or(DEFAULT_OUTPUT_LIMIT)
            .clamp(MIN_OUTPUT_LIMIT, MAX_OUTPUT_LIMIT);
        let mut command = Command::new(&request.command);
        command
            .args(&request.args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for variable in &request.env {
            command.env(&variable.name, &variable.value);
        }
        // Run each terminal in its own process group so descendants of a
        // wrapper command (shell, pipeline) can be killed with it.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start `{}`: {error}", request.command))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let buffer = Arc::new(Mutex::new(TerminalBuffer::new(limit)));
        let mut readers = Vec::new();
        for stream in [stdout.map(Reader::Stdout), stderr.map(Reader::Stderr)]
            .into_iter()
            .flatten()
        {
            let buffer = Arc::clone(&buffer);
            readers.push(tokio::spawn(async move {
                let mut reader = stream.into_reader();
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => buffer.lock().append(&chunk[..read]),
                    }
                }
            }));
        }

        let child = Arc::new(Mutex::new(child));
        let (exit_tx, exit_rx) = watch::channel(None);
        let waiter = Arc::clone(&child);
        tokio::spawn(async move {
            let status = loop {
                match waiter.lock().try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {}
                    Err(_) => break None,
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            let Some(status) = status else {
                return;
            };
            // The process has exited, but the pipe readers may still be
            // appending the final bytes. Publish the exit status only after
            // they observed EOF so an `output()` call following
            // `wait_for_exit()` cannot miss the tail of the output.
            drain_readers(readers).await;
            let _ = exit_tx.send(Some(
                v1s::TerminalExitStatus::new()
                    .exit_code(status.code().map(|code| code.max(0) as u32)),
            ));
        });

        let terminal_id = fleet_core_common::new_id("terminal");
        reservation.commit(
            terminal_id.clone(),
            Arc::new(Terminal {
                child,
                buffer,
                exit: exit_rx,
            }),
        );
        Ok(v1s::CreateTerminalResponse::new(terminal_id))
    }

    /// Read retained output and the exit status, when the command finished.
    pub fn output(&self, terminal_id: &str) -> Result<v1s::TerminalOutputResponse, String> {
        let terminal = self.terminal(terminal_id)?;
        let (output, truncated) = terminal.buffer.lock().snapshot();
        let exit_status = terminal.exit.borrow().clone();
        Ok(v1s::TerminalOutputResponse::new(output, truncated).exit_status(exit_status))
    }

    /// Wait until the command exits.
    pub async fn wait_for_exit(
        &self,
        terminal_id: &str,
    ) -> Result<v1s::TerminalExitStatus, String> {
        let mut exit = self.terminal(terminal_id)?.exit.clone();
        let wait = async {
            loop {
                if let Some(status) = exit.borrow().clone() {
                    return status;
                }
                if exit.changed().await.is_err() {
                    // The watcher task owns the sender; if it is gone the
                    // child was reaped or the manager dropped.
                    if let Some(status) = exit.borrow().clone() {
                        return status;
                    }
                    return v1s::TerminalExitStatus::new();
                }
            }
        };
        match tokio::time::timeout(WAIT_TIMEOUT, wait).await {
            Ok(status) => Ok(status),
            Err(_) => Err(format!(
                "terminal `{terminal_id}` did not exit within {} seconds; poll output or kill it",
                WAIT_TIMEOUT.as_secs()
            )),
        }
    }

    /// Ask the process to terminate.
    pub fn kill(&self, terminal_id: &str) -> Result<(), String> {
        let terminal = self.terminal(terminal_id)?;
        let result = terminate(&mut terminal.child.lock())
            .map_err(|error| format!("failed to kill terminal `{terminal_id}`: {error}"));
        result
    }

    /// Remove a terminal and release its resources.
    pub fn release(&self, terminal_id: &str) -> Result<(), String> {
        let terminal = self
            .slots
            .lock()
            .terminals
            .remove(terminal_id)
            .ok_or_else(|| format!("unknown terminal `{terminal_id}`"))?;
        let _ = terminate(&mut terminal.child.lock());
        Ok(())
    }

    /// Drop every terminal (session shutdown or connection loss).
    pub fn clear(&self) {
        let terminals: Vec<_> = {
            let mut slots = self.slots.lock();
            slots.generation = slots.generation.wrapping_add(1);
            slots.terminals.drain().map(|(_, t)| t).collect()
        };
        for terminal in terminals {
            let _ = terminate(&mut terminal.child.lock());
        }
    }

    fn terminal(&self, terminal_id: &str) -> Result<Arc<Terminal>, String> {
        self.slots
            .lock()
            .terminals
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| format!("unknown terminal `{terminal_id}`"))
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Wait for the pipe readers to observe EOF, bounded so a descendant holding
/// a pipe open cannot delay exit reporting indefinitely.
async fn drain_readers(readers: Vec<tokio::task::JoinHandle<()>>) {
    let drain = async {
        for reader in readers {
            let _ = reader.await;
        }
    };
    let _ = tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, drain).await;
}

/// Kill a terminal and every process in its process group.
fn terminate(child: &mut Child) -> std::io::Result<()> {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return Ok(());
    }
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // `process_group(0)` made the child a group leader, so the negative
        // pid targets the whole group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    match child.start_kill() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

struct Terminal {
    child: Arc<Mutex<Child>>,
    buffer: Arc<Mutex<TerminalBuffer>>,
    exit: watch::Receiver<Option<v1s::TerminalExitStatus>>,
}

enum Reader {
    Stdout(tokio::process::ChildStdout),
    Stderr(tokio::process::ChildStderr),
}

impl Reader {
    fn into_reader(self) -> Box<dyn AsyncRead + Unpin + Send> {
        match self {
            Self::Stdout(stdout) => Box::new(stdout),
            Self::Stderr(stderr) => Box::new(stderr),
        }
    }
}

/// Bounded byte buffer that truncates from the beginning at a UTF-8 boundary.
struct TerminalBuffer {
    /// A deque rather than a `Vec` so dropping the oldest bytes is proportional
    /// to the discarded prefix instead of shifting the whole retained window.
    bytes: VecDeque<u8>,
    truncated: bool,
    limit: usize,
}

impl TerminalBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit.min(64 * 1024)),
            truncated: false,
            limit,
        }
    }

    fn append(&mut self, data: &[u8]) {
        self.bytes.extend(data);
        if self.bytes.len() > self.limit {
            let mut cut = self.bytes.len() - self.limit;
            // Do not start the retained window inside a multibyte character.
            while cut < self.bytes.len() && (self.bytes[cut] & 0xC0) == 0x80 {
                cut += 1;
            }
            self.bytes.drain(..cut);
            self.truncated = true;
        }
    }

    fn snapshot(&self) -> (String, bool) {
        let (front, back) = self.bytes.as_slices();
        let text = if back.is_empty() {
            String::from_utf8_lossy(front).into_owned()
        } else {
            let mut bytes = Vec::with_capacity(front.len() + back.len());
            bytes.extend_from_slice(front);
            bytes.extend_from_slice(back);
            String::from_utf8_lossy(&bytes).into_owned()
        };
        (text, self.truncated)
    }
}

/// Resolve an existing directory and prove it stays inside `root`.
async fn resolve_terminal_cwd(root: &str, requested: &str) -> Result<PathBuf, String> {
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|error| format!("workspace root {root} is invalid: {error}"))?;
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    let canonical = tokio::fs::canonicalize(&candidate)
        .await
        .map_err(|error| format!("terminal cwd {} is invalid: {error}", candidate.display()))?;
    if !canonical.starts_with(&root) {
        return Err(format!("path {requested} escapes the workspace"));
    }
    if !tokio::fs::metadata(&canonical)
        .await
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        return Err(format!(
            "terminal cwd {} is not a directory",
            canonical.display()
        ));
    }
    Ok(canonical)
}

/// Validate that a terminal request belongs to this session.
pub fn request_matches_session(session: &AcpSessionHandle, session_id: &str) -> bool {
    session.native_session_id.lock().as_deref() == Some(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_truncates_from_the_start_at_a_char_boundary() {
        let mut buffer = TerminalBuffer::new(8);
        buffer.append(b"ab");
        buffer.append(b"cd");
        assert_eq!(buffer.snapshot(), ("abcd".to_string(), false));
        buffer.append(b"efghij");
        let (output, truncated) = buffer.snapshot();
        assert!(truncated);
        assert_eq!(output, "cdefghij");
        assert!(output.len() <= 8);
    }

    #[test]
    fn multibyte_characters_are_not_split() {
        let mut buffer = TerminalBuffer::new(6);
        buffer.append(b"aaaa");
        buffer.append("éé".as_bytes()); // 4 bytes; the retained window would start mid-char
        let (output, truncated) = buffer.snapshot();
        assert!(truncated);
        assert!(!output.is_empty());
        assert!(output.is_char_boundary(0));
        assert!(output.len() <= 6);
    }

    #[test]
    fn repeated_appends_retain_exactly_the_trailing_window() {
        let mut buffer = TerminalBuffer::new(5);
        for byte in b"0123456789" {
            buffer.append(&[*byte]);
            assert!(buffer.bytes.len() <= 5);
        }
        assert_eq!(buffer.snapshot(), ("56789".to_string(), true));

        // A single oversized chunk is trimmed from the front in the same way.
        let mut buffer = TerminalBuffer::new(4);
        buffer.append(b"abcdef");
        assert_eq!(buffer.snapshot(), ("cdef".to_string(), true));
    }

    #[tokio::test]
    async fn runs_a_command_and_reports_exit() {
        let temp = tempfile::tempdir().unwrap();
        let session = test_session(temp.path().to_string_lossy().to_string());
        let manager = TerminalManager::new();
        let created = manager
            .create(
                &session,
                &v1s::CreateTerminalRequest::new("sess_acp_1", "sh")
                    .args(vec!["-c".into(), "printf hello".into()]),
            )
            .await
            .expect("create terminal");
        let terminal_id = created.terminal_id.to_string();
        let exit = manager.wait_for_exit(&terminal_id).await.expect("wait");
        assert_eq!(exit.exit_code, Some(0));
        let output = manager.output(&terminal_id).expect("output");
        assert_eq!(output.output, "hello");
        assert!(manager.release(&terminal_id).is_ok());
        assert!(manager.output(&terminal_id).is_err());
        manager.clear();
    }

    #[tokio::test]
    async fn output_after_wait_contains_the_whole_stream() {
        let temp = tempfile::tempdir().unwrap();
        let session = test_session(temp.path().to_string_lossy().to_string());
        let manager = TerminalManager::new();
        // More than a pipe buffer, so the readers can still be draining when
        // the child exits. Publishing the exit status before they observe EOF
        // would make `output()` miss the tail of the stream.
        let bytes = 512 * 1024;
        let script = format!("head -c {bytes} /dev/zero | tr '\\000' x");
        let created = manager
            .create(
                &session,
                &v1s::CreateTerminalRequest::new("sess_acp_1", "sh")
                    .args(vec!["-c".into(), script]),
            )
            .await
            .expect("create terminal");
        let terminal_id = created.terminal_id.to_string();

        let exit = manager.wait_for_exit(&terminal_id).await.expect("wait");
        assert_eq!(exit.exit_code, Some(0));
        let output = manager.output(&terminal_id).expect("output");
        assert!(!output.truncated);
        assert_eq!(output.output.len(), bytes);
        assert!(output.output.bytes().all(|byte| byte == b'x'));
        manager.clear();
    }

    #[tokio::test]
    async fn kill_terminates_the_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let session = test_session(temp.path().to_string_lossy().to_string());
        let manager = TerminalManager::new();
        let pid_file = temp.path().join("child.pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let created = manager
            .create(
                &session,
                &v1s::CreateTerminalRequest::new("sess_acp_1", "sh")
                    .args(vec!["-c".into(), script]),
            )
            .await
            .expect("create terminal");
        let terminal_id = created.terminal_id.to_string();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let descendant: i32 = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = text.trim().parse() {
                    break pid;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the terminal did not report its child pid"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };

        manager.kill(&terminal_id).expect("kill terminal");
        let exit = manager.wait_for_exit(&terminal_id).await.expect("wait");
        assert!(exit.exit_code.is_none(), "expected a signal exit");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let alive = unsafe { libc::kill(descendant, 0) } == 0;
            if !alive {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "descendant process {descendant} survived terminal kill"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        manager.clear();
    }

    #[tokio::test]
    async fn rejects_a_cwd_outside_the_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let session = test_session(temp.path().to_string_lossy().to_string());
        let manager = TerminalManager::new();
        let error = manager
            .create(
                &session,
                &v1s::CreateTerminalRequest::new("sess_acp_1", "true").cwd("/etc"),
            )
            .await
            .expect_err("outside cwd must be rejected");
        assert!(error.contains("escapes the workspace"));
    }

    #[tokio::test]
    async fn failed_creates_release_their_slot() {
        let temp = tempfile::tempdir().unwrap();
        let session = test_session(temp.path().to_string_lossy().to_string());
        let manager = TerminalManager::new();
        for _ in 0..MAX_TERMINALS {
            assert!(manager
                .create(
                    &session,
                    &v1s::CreateTerminalRequest::new("sess_acp_1", "true").cwd("/etc"),
                )
                .await
                .is_err());
        }
        let created = manager
            .create(
                &session,
                &v1s::CreateTerminalRequest::new("sess_acp_1", "true"),
            )
            .await
            .expect("failed creates must not consume slots");
        assert!(manager.release(&created.terminal_id.to_string()).is_ok());
        manager.clear();
    }

    #[tokio::test]
    async fn concurrent_creates_respect_the_terminal_limit() {
        let temp = tempfile::tempdir().unwrap();
        let session = Arc::new(test_session(temp.path().to_string_lossy().to_string()));
        let manager = Arc::new(TerminalManager::new());
        let mut tasks = Vec::new();
        for _ in 0..(MAX_TERMINALS * 2) {
            let manager = Arc::clone(&manager);
            let session = Arc::clone(&session);
            tasks.push(tokio::spawn(async move {
                manager
                    .create(
                        &session,
                        &v1s::CreateTerminalRequest::new("sess_acp_1", "sleep")
                            .args(vec!["1".into()]),
                    )
                    .await
            }));
        }
        let mut created = 0;
        for task in tasks {
            if task.await.expect("create task").is_ok() {
                created += 1;
            }
        }
        assert_eq!(
            created, MAX_TERMINALS,
            "concurrent creates must never exceed the slot limit"
        );
        manager.clear();
    }

    fn test_session(workspace_root: String) -> AcpSessionHandle {
        let emitter = crate::driver::acp::tests::test_emitter("s1");
        let session = AcpSessionHandle::new(
            "s1",
            "i1",
            emitter,
            Duration::from_secs(5),
            Duration::from_secs(5),
        );
        *session.workspace_root.lock() = workspace_root.clone();
        *session.session_cwd.lock() = workspace_root;
        *session.native_session_id.lock() = Some("sess_acp_1".to_string());
        Arc::try_unwrap(session).ok().expect("unique session")
    }
}

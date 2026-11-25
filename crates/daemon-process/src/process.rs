//! Long-lived child process management.

use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{broadcast, mpsc, watch, Mutex};

use crate::command::CommandSpec;
use crate::error::{ProcessError, ProcessResult};
use crate::output::{ProcessEvent, OUTPUT_CHUNK_SIZE};

/// Capacity of the broadcast channel used for process output events.
const EVENT_CHANNEL_CAPACITY: usize = 1024;
/// How long [`ManagedProcess::terminate`] waits after `SIGTERM` before it
/// escalates to `SIGKILL`.
const TERMINATE_GRACE: Duration = Duration::from_millis(500);
/// Upper bound on how long the monitor waits for stdout/stderr to reach EOF
/// after the direct child exits. This prevents grandchildren that inherited
/// the pipes from delaying exit notification indefinitely.
pub(crate) const OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
enum Signal {
    Interrupt,
    Terminate,
    Kill,
}

#[derive(Debug, Clone)]
enum ExitOutcome {
    Exited(ExitStatus),
    Failed(String),
}

impl ExitOutcome {
    fn into_result(self) -> ProcessResult<ExitStatus> {
        match self {
            ExitOutcome::Exited(status) => Ok(status),
            ExitOutcome::Failed(message) => Err(ProcessError::Other(message)),
        }
    }
}

/// A spawned child process with piped standard I/O.
///
/// The process is started in its own process group on Unix, and a background
/// monitor task reaps it and broadcasts output events. Dropping the handle
/// kills the entire process group if the child is still running, so a daemon
/// shutdown does not leave orphaned agent processes behind.
///
/// ```no_run
/// # async fn example() -> fleet_daemon_process::ProcessResult<()> {
/// use fleet_daemon_process::{CommandSpec, ManagedProcess};
///
/// let mut process = ManagedProcess::spawn(
///     CommandSpec::new("sh").args(["-c", "printf hello"]),
/// )
/// .await?;
/// let status = process.wait().await?;
/// assert!(status.success());
/// # Ok(())
/// # }
/// ```
pub struct ManagedProcess {
    pid: Option<u32>,
    stdin: Mutex<Option<ChildStdin>>,
    events: broadcast::Sender<ProcessEvent>,
    signal_tx: mpsc::UnboundedSender<Signal>,
    exit_rx: watch::Receiver<Option<ExitOutcome>>,
    running: Arc<AtomicBool>,
}

impl ManagedProcess {
    /// Spawn a process with piped stdin, stdout, and stderr.
    ///
    /// On Unix the child becomes the leader of a new process group. Output
    /// emitted before [`Self::subscribe`] is not replayed; use
    /// [`Self::spawn_with_events`] when the very first bytes matter.
    pub async fn spawn(spec: CommandSpec) -> ProcessResult<Self> {
        let (process, _events) = Self::spawn_with_events(spec).await?;
        Ok(process)
    }

    /// Spawn a process and return a receiver that is already subscribed, so
    /// early stdout/stderr (and the `Started` event) cannot be lost.
    pub async fn spawn_with_events(
        spec: CommandSpec,
    ) -> ProcessResult<(Self, broadcast::Receiver<ProcessEvent>)> {
        let mut command = spec.to_tokio_command();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|err| {
            ProcessError::Spawn(format!("failed to spawn `{}`: {err}", spec.program))
        })?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let (events, initial_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        // `initial_rx` exists before the reader tasks start, so `Started` and
        // the first output chunk are retained for the caller.
        let _ = events.send(ProcessEvent::Started { pid });

        let (signal_tx, mut signal_rx) = mpsc::unbounded_channel::<Signal>();
        let running = Arc::new(AtomicBool::new(true));
        let (exit_tx, exit_rx) = watch::channel(None);

        let stdout_events = events.clone();
        let stdout_task = tokio::spawn(async move {
            if let Some(stdout) = stdout {
                read_and_emit(stdout, stdout_events, ProcessEvent::Stdout).await;
            }
        });
        let stderr_events = events.clone();
        let stderr_task = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                read_and_emit(stderr, stderr_events, ProcessEvent::Stderr).await;
            }
        });

        let monitor_events = events.clone();
        let monitor_exit_tx = exit_tx;
        let monitor_running = Arc::clone(&running);
        tokio::spawn(async move {
            let outcome = loop {
                tokio::select! {
                    status = child.wait() => break status,
                    signal = signal_rx.recv() => {
                        match signal {
                            Some(signal) => apply_signal(pid, &mut child, signal),
                            None => break child.wait().await,
                        }
                    }
                }
            };

            // Drain both streams before announcing exit so subscribers observe
            // output before the terminal event. The drain is bounded so a
            // daemonized grandchild holding the pipes open cannot keep the
            // process marked as running forever.
            let drain = async {
                let _ = stdout_task.await;
                let _ = stderr_task.await;
            };
            let _ = tokio::time::timeout(OUTPUT_DRAIN_GRACE, drain).await;
            monitor_running.store(false, Ordering::SeqCst);

            match outcome {
                Ok(status) => {
                    let _ = monitor_events.send(ProcessEvent::Exited {
                        code: status.code(),
                        success: status.success(),
                    });
                    let _ = monitor_exit_tx.send(Some(ExitOutcome::Exited(status)));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to wait for child process");
                    let _ = monitor_exit_tx.send(Some(ExitOutcome::Failed(err.to_string())));
                }
            }
        });

        Ok((
            Self {
                pid,
                stdin: Mutex::new(stdin),
                events,
                signal_tx,
                exit_rx,
                running,
            },
            initial_rx,
        ))
    }

    /// Operating-system process id, when available.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Write bytes to the child's standard input.
    pub async fn write_stdin(&self, data: &[u8]) -> ProcessResult<()> {
        if !self.is_running() {
            return Err(ProcessError::NotRunning);
        }
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or(ProcessError::NotRunning)?;
        stdin.write_all(data).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Close the child's standard input, signalling EOF.
    pub async fn close_stdin(&self) -> ProcessResult<()> {
        let mut guard = self.stdin.lock().await;
        drop(guard.take());
        Ok(())
    }

    /// Subscribe to process output events.
    ///
    /// Only events emitted after the subscription is created are delivered.
    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.events.subscribe()
    }

    /// Send `SIGINT` to the process group (Unix).
    pub async fn interrupt(&self) -> ProcessResult<()> {
        self.send_signal(Signal::Interrupt)
    }

    /// Send `SIGTERM` to the process group and escalate to `SIGKILL` if the
    /// process does not exit within a short grace period.
    pub async fn terminate(&self) -> ProcessResult<()> {
        if !self.is_running() {
            return Ok(());
        }
        self.send_signal(Signal::Terminate)?;
        match tokio::time::timeout(TERMINATE_GRACE, self.wait_until_exit()).await {
            Ok(Ok(_)) => Ok(()),
            _ => self.send_signal(Signal::Kill),
        }
    }

    /// Send `SIGKILL` to the process group.
    pub async fn kill(&self) -> ProcessResult<()> {
        self.send_signal(Signal::Kill)
    }

    /// Wait for the process to exit and return its status.
    pub async fn wait(&mut self) -> ProcessResult<ExitStatus> {
        self.wait_until_exit().await
    }

    /// Whether the child is still running.
    ///
    /// The flag flips to `false` after the child has been reaped and all
    /// buffered output has been drained.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn send_signal(&self, signal: Signal) -> ProcessResult<()> {
        if !self.is_running() {
            return Ok(());
        }
        // A send failure means the monitor has already finished (for example,
        // the child exited while its output was draining). Signalling is
        // best-effort and idempotent in that situation.
        if self.signal_tx.send(signal).is_err() {
            tracing::debug!("process monitor stopped before signal could be delivered");
        }
        Ok(())
    }

    async fn wait_until_exit(&self) -> ProcessResult<ExitStatus> {
        let mut rx = self.exit_rx.clone();
        loop {
            if let Some(outcome) = rx.borrow().clone() {
                return outcome.into_result();
            }
            if rx.changed().await.is_err() {
                return match rx.borrow().clone() {
                    Some(outcome) => outcome.into_result(),
                    None => Err(ProcessError::NotRunning),
                };
            }
        }
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        #[cfg(unix)]
        if let Some(pid) = self.pid {
            crate::signal::kill_group(pid);
        }

        // Also ask the monitor to kill the direct child (and, on non-Unix
        // platforms, to use `start_kill`).
        let _ = self.signal_tx.send(Signal::Kill);
    }
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn apply_signal(pid: Option<u32>, child: &mut Child, signal: Signal) {
    #[cfg(unix)]
    {
        if let Some(pid) = pid {
            match signal {
                Signal::Interrupt => crate::signal::interrupt_group(pid),
                Signal::Terminate => crate::signal::terminate_group(pid),
                Signal::Kill => crate::signal::kill_group(pid),
            }
            return;
        }
    }

    // Non-Unix fallback (and Unix fallback when no pid is available).
    let _ = child.start_kill();
}

async fn read_and_emit<R>(
    mut reader: R,
    events: broadcast::Sender<ProcessEvent>,
    make_event: fn(Vec<u8>) -> ProcessEvent,
) where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; OUTPUT_CHUNK_SIZE];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => {
                let _ = events.send(make_event(buffer[..n].to_vec()));
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                tracing::debug!(error = %err, "process output stream ended with an error");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    fn sh(script: &str) -> CommandSpec {
        CommandSpec::new("sh").arg("-c").arg(script)
    }

    #[tokio::test]
    async fn stdin_roundtrip_with_cat() {
        let mut process = ManagedProcess::spawn(CommandSpec::new("cat"))
            .await
            .expect("cat should spawn");
        let mut events = process.subscribe();

        process.write_stdin(b"hello stdin").await.expect("write");
        process.close_stdin().await.expect("close stdin");
        let status = process.wait().await.expect("wait");
        assert!(status.success());

        let mut stdout = Vec::new();
        loop {
            match events.try_recv() {
                Ok(ProcessEvent::Stdout(chunk)) => stdout.extend_from_slice(&chunk),
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => continue,
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        assert_eq!(stdout, b"hello stdin");
    }

    #[tokio::test]
    async fn subscribe_receives_exited_event() {
        let process = ManagedProcess::spawn(sh("sleep 0.2; exit 3"))
            .await
            .expect("child should spawn");
        let mut events = process.subscribe();

        let exited = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events.recv().await {
                    Ok(ProcessEvent::Exited { code, success }) => return (code, success),
                    Ok(_) => {}
                    Err(err) => panic!("event channel failed: {err}"),
                }
            }
        })
        .await
        .expect("exited event should arrive");

        assert_eq!(exited, (Some(3), false));
    }

    #[tokio::test]
    async fn is_running_false_after_exit() {
        let mut process = ManagedProcess::spawn(sh("exit 0"))
            .await
            .expect("child should spawn");

        let status = process.wait().await.expect("wait");
        assert!(status.success());
        assert!(!process.is_running());
    }

    #[tokio::test]
    async fn terminate_stops_long_running_process() {
        let mut process = ManagedProcess::spawn(sh("sleep 10"))
            .await
            .expect("child should spawn");
        process.terminate().await.expect("terminate");
        let status = tokio::time::timeout(Duration::from_secs(5), process.wait())
            .await
            .expect("process should exit after terminate")
            .expect("wait should not fail");
        assert!(!status.success());
        assert!(!process.is_running());
    }

    #[tokio::test]
    async fn kill_stops_long_running_process() {
        let mut process = ManagedProcess::spawn(sh("sleep 10"))
            .await
            .expect("child should spawn");
        process.kill().await.expect("kill");
        let _ = tokio::time::timeout(Duration::from_secs(5), process.wait())
            .await
            .expect("process should exit after kill")
            .expect("wait should not fail");
        assert!(!process.is_running());
    }

    #[tokio::test]
    async fn writing_to_closed_stdin_fails() {
        let mut process = ManagedProcess::spawn(sh("true"))
            .await
            .expect("child should spawn");
        process.close_stdin().await.expect("close stdin");
        assert!(matches!(
            process.write_stdin(b"x").await,
            Err(ProcessError::NotRunning)
        ));
        let _ = process.wait().await;
    }
}

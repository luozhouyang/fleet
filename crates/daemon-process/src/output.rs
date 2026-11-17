//! Output events and one-shot command capture.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::command::CommandSpec;
use crate::error::{ProcessError, ProcessResult};

/// Maximum number of bytes read from a pipe in a single chunk.
pub(crate) const OUTPUT_CHUNK_SIZE: usize = 8 * 1024;

/// Events broadcast by a [`ManagedProcess`](crate::ManagedProcess).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    /// The process was spawned.
    Started {
        /// Operating-system process id, when available.
        pid: Option<u32>,
    },
    /// A chunk of data was read from standard output.
    Stdout(Vec<u8>),
    /// A chunk of data was read from standard error.
    Stderr(Vec<u8>),
    /// The process exited.
    Exited {
        /// Exit code, or `None` when terminated by a signal.
        code: Option<i32>,
        /// Whether the exit status reports success.
        success: bool,
    },
}

/// Result of [`run_capture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedOutput {
    /// Exit code, or `None` when the process was terminated by a signal.
    pub status: Option<i32>,
    /// Whether the process exited successfully. A non-zero exit is not an
    /// error; callers should inspect this field and `status` instead.
    pub success: bool,
    /// Captured standard output, capped at the requested limit.
    pub stdout: Vec<u8>,
    /// Captured standard error, capped at the requested limit.
    pub stderr: Vec<u8>,
    /// Whether either stream exceeded the output cap.
    pub truncated: bool,
}

/// Run a command to completion and capture its output.
///
/// Both streams are drained concurrently and capped at `max_output_bytes`
/// each. When the combined output exceeds the cap the excess is discarded and
/// `truncated` is set. If the command does not finish within `timeout` its
/// process group is killed and [`ProcessError::Timeout`] is returned.
///
/// A non-zero exit status is **not** an error: the captured output is returned
/// with `success = false`. Only spawn, I/O, and timeout failures produce an
/// error.
pub async fn run_capture(
    spec: CommandSpec,
    timeout: Duration,
    max_output_bytes: usize,
) -> ProcessResult<CapturedOutput> {
    let mut command: Command = spec.to_tokio_command();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|err| ProcessError::Spawn(format!("failed to spawn `{}`: {err}", spec.program)))?;
    #[cfg(unix)]
    let pid = child.id();

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessError::Other("stdout pipe was not captured".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessError::Other("stderr pipe was not captured".to_string()))?;

    let stdout_task = tokio::spawn(read_capped(stdout, max_output_bytes));
    let stderr_task = tokio::spawn(read_capped(stderr, max_output_bytes));

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => {
            stdout_task.abort();
            stderr_task.abort();
            return Err(ProcessError::Io(err));
        }
        Err(_) => {
            // Kill the whole process tree, then reap the direct child.
            #[cfg(unix)]
            if let Some(pid) = pid {
                crate::signal::kill_group(pid);
            }
            let _ = child.kill().await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ProcessError::Timeout(timeout));
        }
    };

    let (stdout, stdout_truncated) = join_reader(stdout_task).await?;
    let (stderr, stderr_truncated) = join_reader(stderr_task).await?;

    Ok(CapturedOutput {
        status: status.code(),
        success: status.success(),
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated,
    })
}

async fn join_reader(
    handle: JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
) -> ProcessResult<(Vec<u8>, bool)> {
    match handle.await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(err)) => Err(ProcessError::Io(err)),
        Err(err) => Err(ProcessError::Other(format!(
            "process output reader task failed: {err}"
        ))),
    }
}

async fn read_capped<R>(mut reader: R, max_bytes: usize) -> std::io::Result<(Vec<u8>, bool)>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; OUTPUT_CHUNK_SIZE];
    let mut output = Vec::new();
    let mut truncated = false;

    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => {
                let remaining = max_bytes.saturating_sub(output.len());
                if remaining == 0 {
                    truncated = true;
                } else {
                    let keep = remaining.min(n);
                    output.extend_from_slice(&buffer[..keep]);
                    if keep < n {
                        truncated = true;
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }

    Ok((output, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(script: &str) -> CommandSpec {
        CommandSpec::new("sh").arg("-c").arg(script)
    }

    #[tokio::test]
    async fn captures_stdout() {
        let output = run_capture(spec("printf 'hello'"), Duration::from_secs(5), 1024)
            .await
            .expect("capture should succeed");
        assert!(output.success);
        assert_eq!(output.status, Some(0));
        assert_eq!(output.stdout, b"hello");
        assert!(output.stderr.is_empty());
        assert!(!output.truncated);
    }

    #[tokio::test]
    async fn captures_stderr() {
        let output = run_capture(spec("printf 'oops' >&2"), Duration::from_secs(5), 1024)
            .await
            .expect("capture should succeed");
        assert!(output.success);
        assert_eq!(output.stderr, b"oops");
        assert!(output.stdout.is_empty());
    }

    #[tokio::test]
    async fn nonzero_exit_is_not_an_error() {
        let output = run_capture(
            spec("printf 'bad' >&2; exit 7"),
            Duration::from_secs(5),
            1024,
        )
        .await
        .expect("non-zero exit must still return output");
        assert!(!output.success);
        assert_eq!(output.status, Some(7));
        assert_eq!(output.stderr, b"bad");
    }

    #[tokio::test]
    async fn truncates_output_at_cap() {
        let output = run_capture(spec("printf 'abcdefghij'"), Duration::from_secs(5), 4)
            .await
            .expect("capture should succeed");
        assert!(output.success);
        assert_eq!(output.stdout, b"abcd");
        assert!(output.truncated);
    }

    #[tokio::test]
    async fn timeout_kills_long_running_process() {
        let started = std::time::Instant::now();
        let error = run_capture(spec("sleep 10"), Duration::from_millis(150), 1024)
            .await
            .expect_err("timeout should be reported as an error");
        assert!(matches!(error, ProcessError::Timeout(_)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn zero_cap_truncates_everything() {
        let output = run_capture(spec("printf 'abc'"), Duration::from_secs(5), 0)
            .await
            .expect("capture should succeed");
        assert!(output.stdout.is_empty());
        assert!(output.truncated);
    }
}

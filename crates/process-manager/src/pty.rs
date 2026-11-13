//! Pseudo-terminal backed process management for terminal-compatible agents.

use std::io::{Read, Write};
use std::sync::Mutex;

use portable_pty::{native_pty_system, Child, MasterPty, PtySize};
use tokio::sync::mpsc::{self, Receiver};

use crate::command::CommandSpec;
use crate::error::{ProcessError, ProcessResult};

/// Capacity of the channel that forwards PTY output.
const PTY_CHANNEL_CAPACITY: usize = 256;
/// Maximum number of bytes read from the PTY in one chunk.
const PTY_CHUNK_SIZE: usize = 8 * 1024;

/// A child process attached to a pseudo-terminal.
///
/// A dedicated thread reads the master side and forwards raw bytes through an
/// mpsc channel. Use [`PtyProcess::reader`] once to obtain the receiving end.
pub struct PtyProcess {
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    reader: Mutex<Option<Receiver<Vec<u8>>>>,
}

impl PtyProcess {
    /// Spawn `spec` inside a new pseudo-terminal of the given size.
    pub fn spawn(spec: CommandSpec, cols: u16, rows: u16) -> ProcessResult<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| ProcessError::Pty(format!("failed to open pty: {err}")))?;

        let portable_pty::PtyPair { master, slave } = pair;
        let mut child = slave
            .spawn_command(spec.to_pty_command())
            .map_err(|err| ProcessError::Pty(format!("failed to spawn in pty: {err}")))?;
        // The slave handle must be closed in the parent or the reader will
        // never observe EOF.
        drop(slave);

        let mut pty_reader = master
            .try_clone_reader()
            .map_err(|err| ProcessError::Pty(format!("failed to clone pty reader: {err}")))?;
        let writer = master
            .take_writer()
            .map_err(|err| ProcessError::Pty(format!("failed to take pty writer: {err}")))?;

        let (tx, rx) = mpsc::channel(PTY_CHANNEL_CAPACITY);
        let reader_thread = std::thread::Builder::new()
            .name("agent-fleet-pty-reader".to_string())
            .spawn(move || {
                let mut buffer = [0u8; PTY_CHUNK_SIZE];
                loop {
                    match pty_reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.blocking_send(buffer[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            });
        if let Err(err) = reader_thread {
            let _ = child.kill();
            return Err(ProcessError::Pty(format!(
                "failed to start pty reader: {err}"
            )));
        }

        Ok(Self {
            child,
            master,
            writer,
            reader: Mutex::new(Some(rx)),
        })
    }

    /// Write raw bytes to the pseudo-terminal.
    pub fn write(&mut self, data: &[u8]) -> ProcessResult<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Resize the pseudo-terminal window.
    pub fn resize(&mut self, cols: u16, rows: u16) -> ProcessResult<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| ProcessError::Pty(format!("failed to resize pty: {err}")))
    }

    /// Take the channel that yields raw PTY output.
    ///
    /// The channel can only be taken once; subsequent calls return an
    /// immediately closed receiver.
    pub fn reader(&self) -> Receiver<Vec<u8>> {
        let mut guard = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.take() {
            Some(receiver) => receiver,
            None => {
                let (_tx, rx) = mpsc::channel(1);
                rx
            }
        }
    }

    /// Terminate the child process.
    pub fn kill(&mut self) -> ProcessResult<()> {
        self.child.kill().map_err(ProcessError::Io)
    }

    /// Wait for the child to exit and return its exit code.
    pub fn wait(&mut self) -> ProcessResult<Option<i32>> {
        let status = self.child.wait().map_err(ProcessError::Io)?;
        Ok(Some(status.exit_code() as i32))
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        // Best effort: never leave an orphaned interactive child behind.
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sh(script: &str) -> CommandSpec {
        CommandSpec::new("sh").arg("-c").arg(script)
    }

    #[test]
    fn pty_forwards_output() {
        let mut process =
            PtyProcess::spawn(sh("printf 'pty-ok'"), 80, 24).expect("pty process should spawn");
        let mut reader = process.reader();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut collected = Vec::new();
        while std::time::Instant::now() < deadline {
            match reader.blocking_recv() {
                Some(chunk) => {
                    collected.extend_from_slice(&chunk);
                    if String::from_utf8_lossy(&collected).contains("pty-ok") {
                        break;
                    }
                }
                None => break,
            }
        }

        assert!(
            String::from_utf8_lossy(&collected).contains("pty-ok"),
            "unexpected pty output: {:?}",
            String::from_utf8_lossy(&collected)
        );

        let code = process.wait().expect("wait should succeed");
        assert_eq!(code, Some(0));
    }

    #[test]
    fn resize_and_write_do_not_fail() {
        let mut process =
            PtyProcess::spawn(sh("sleep 10"), 80, 24).expect("pty process should spawn");
        process.resize(120, 40).expect("resize");
        process.write(b"hello\n").expect("write");
        let _ = process.kill();
        let _ = process.wait();
    }
}

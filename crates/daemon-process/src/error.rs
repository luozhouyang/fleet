//! Errors produced by the process manager.

use std::time::Duration;

use fleet_core_common::{error_codes, ApiError};

/// Errors that can occur while spawning or controlling a child process.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The process could not be spawned.
    #[error("failed to spawn process: {0}")]
    Spawn(String),
    /// An I/O operation on the process failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The process has already exited or its standard input is closed.
    #[error("process is not running")]
    NotRunning,
    /// The command did not finish within the configured timeout.
    #[error("process timed out after {0:?}")]
    Timeout(Duration),
    /// A pseudo-terminal operation failed.
    #[error("pty error: {0}")]
    Pty(String),
    /// Any other process manager failure.
    #[error("{0}")]
    Other(String),
}

/// Convenience alias used throughout this crate.
pub type ProcessResult<T> = Result<T, ProcessError>;

impl From<ProcessError> for ApiError {
    fn from(value: ProcessError) -> Self {
        let code = match value {
            ProcessError::Timeout(_) => error_codes::TIMEOUT,
            _ => error_codes::INTERNAL,
        };
        ApiError::new(code, value.to_string())
    }
}

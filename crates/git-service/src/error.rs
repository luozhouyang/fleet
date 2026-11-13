//! Error type produced by the Git service.

use std::path::PathBuf;
use std::time::Duration;

use agent_fleet_common::{error_codes, ApiError};
use agent_fleet_process_manager::ProcessError;

/// Convenience result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, GitError>;

/// Errors returned by Git operations.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The requested directory is not inside a Git work tree.
    #[error("not a git repository: {}", .0.display())]
    NotARepository(PathBuf),

    /// A Git command did not finish within the configured timeout.
    #[error("git command timed out after {0:?}")]
    Timeout(Duration),

    /// Git produced output that could not be parsed.
    #[error("invalid git output: {0}")]
    InvalidOutput(String),

    /// A Git command exited with a non-zero status.
    #[error("git command failed: {0}")]
    CommandFailed(String),

    /// A caller-provided path or revision was rejected.
    #[error("invalid path or revision: {0}")]
    InvalidPath(String),

    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<ProcessError> for GitError {
    fn from(error: ProcessError) -> Self {
        match error {
            ProcessError::Timeout(timeout) => Self::Timeout(timeout),
            ProcessError::Io(error) => Self::Io(error),
            ProcessError::Spawn(message) | ProcessError::Pty(message) => {
                Self::CommandFailed(message)
            }
            ProcessError::NotRunning | ProcessError::Other(_) => {
                Self::CommandFailed(error.to_string())
            }
        }
    }
}

impl From<GitError> for ApiError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::NotARepository(ref path) => {
                ApiError::new(error_codes::NOT_A_REPOSITORY, error.to_string())
                    .with_details(serde_json::json!({ "path": path.to_string_lossy() }))
            }
            GitError::Timeout(_) => ApiError::new(error_codes::TIMEOUT, error.to_string()),
            GitError::InvalidOutput(message) | GitError::InvalidPath(message) => {
                ApiError::invalid(message)
            }
            GitError::CommandFailed(message) => ApiError::internal(message),
            GitError::Io(error) => match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                    ApiError::not_found(error.to_string())
                }
                std::io::ErrorKind::PermissionDenied => {
                    ApiError::new(error_codes::PATH_DENIED, error.to_string())
                }
                _ => ApiError::internal(error.to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_error_codes() {
        let error: ApiError = GitError::NotARepository(PathBuf::from("/tmp")).into();
        assert_eq!(error.code, error_codes::NOT_A_REPOSITORY);

        let error: ApiError = GitError::Timeout(Duration::from_secs(1)).into();
        assert_eq!(error.code, error_codes::TIMEOUT);

        let error: ApiError = GitError::InvalidOutput("bad".into()).into();
        assert_eq!(error.code, error_codes::INVALID_REQUEST);

        let error: ApiError = GitError::CommandFailed("boom".into()).into();
        assert_eq!(error.code, error_codes::INTERNAL);
    }

    #[test]
    fn maps_process_timeout() {
        let error = GitError::from(ProcessError::Timeout(Duration::from_secs(2)));
        assert!(matches!(error, GitError::Timeout(_)));
    }
}

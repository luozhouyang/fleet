//! Error type produced by the workspace crate.

use fleet_core_common::{error_codes, ApiError};
use fleet_daemon_storage::StorageError;

/// Convenience result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, WorkspaceError>;

/// Errors returned by workspace operations.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// The requested workspace is not registered on this machine.
    #[error("workspace not found: {0}")]
    WorkspaceNotFound(String),

    /// A path escaped the workspace root or contained a forbidden segment.
    #[error("path denied: {0}")]
    PathDenied(String),

    /// A workspace-relative path does not exist on disk.
    #[error("not found: {0}")]
    NotFound(String),

    /// The request was structurally invalid.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// A file exceeds the configured read limit.
    #[error("file too large: {path} is {size} bytes, limit is {limit}")]
    FileTooLarge {
        /// Workspace-relative file path.
        path: String,
        /// Full size reported by the filesystem.
        size: u64,
        /// Limit that applied to the read.
        limit: u64,
    },

    /// A binary file was requested through the text API.
    #[error("binary file: {0}")]
    BinaryFile(String),

    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// The storage layer failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<WorkspaceError> for ApiError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::WorkspaceNotFound(message) | WorkspaceError::NotFound(message) => {
                ApiError::new(error_codes::NOT_FOUND, message)
            }
            WorkspaceError::PathDenied(message) => ApiError::new(error_codes::PATH_DENIED, message),
            WorkspaceError::InvalidRequest(message) => ApiError::invalid(message),
            WorkspaceError::FileTooLarge { path, size, limit } => ApiError::new(
                error_codes::FILE_TOO_LARGE,
                format!("{path} is {size} bytes, limit is {limit}"),
            ),
            WorkspaceError::BinaryFile(message) => ApiError::new(error_codes::BINARY_FILE, message),
            WorkspaceError::Io(error) => match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                    ApiError::new(error_codes::NOT_FOUND, error.to_string())
                }
                std::io::ErrorKind::PermissionDenied => {
                    ApiError::new(error_codes::PATH_DENIED, error.to_string())
                }
                _ => ApiError::internal(error.to_string()),
            },
            WorkspaceError::Storage(error) => ApiError::from(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_mapped() {
        let denied: ApiError = WorkspaceError::PathDenied("../../etc".into()).into();
        assert_eq!(denied.code, error_codes::PATH_DENIED);

        let missing: ApiError = WorkspaceError::NotFound("nope".into()).into();
        assert_eq!(missing.code, error_codes::NOT_FOUND);

        let too_large: ApiError = WorkspaceError::FileTooLarge {
            path: "big.bin".into(),
            size: 10,
            limit: 5,
        }
        .into();
        assert_eq!(too_large.code, error_codes::FILE_TOO_LARGE);

        let binary: ApiError = WorkspaceError::BinaryFile("logo.png".into()).into();
        assert_eq!(binary.code, error_codes::BINARY_FILE);
    }
}

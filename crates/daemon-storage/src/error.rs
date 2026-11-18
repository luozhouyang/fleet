//! Error type shared by every storage operation.

use fleet_core_common::ApiError;

/// Errors produced by the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// An error returned by the SQLite driver.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// An error while applying embedded migrations.
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    /// A JSON column could not be serialized.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A filesystem operation failed (for example creating the database parent).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A row contained data that cannot be interpreted.
    #[error("invalid data: {0}")]
    InvalidData(String),
}

/// Convenience result alias used across the crate.
pub type Result<T> = std::result::Result<T, StorageError>;

impl StorageError {
    /// Whether retrying the same write may succeed later.
    ///
    /// SQLite busy/locked, I/O errors and pool timeouts are transient and
    /// deserve a retry. Constraint violations, invalid data and serialization
    /// failures are deterministic: retrying them forever would stall every
    /// later event behind one poison record.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Database(sqlx::Error::Database(error)) => {
                // SQLite extended result codes keep the primary code in the
                // low byte. BUSY (5), LOCKED (6), IOERR (10) and FULL (13)
                // can clear up; constraint (19/787) and similar cannot.
                error
                    .code()
                    .and_then(|code| code.parse::<i32>().ok())
                    .is_some_and(|code| matches!(code & 0xff, 5 | 6 | 10 | 13))
            }
            Self::Database(sqlx::Error::Io(_))
            | Self::Database(sqlx::Error::PoolTimedOut)
            | Self::Io(_) => true,
            Self::Database(_)
            | Self::Migration(_)
            | Self::Serialization(_)
            | Self::InvalidData(_) => false,
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match &error {
            StorageError::InvalidData(message) => ApiError::invalid(message.clone()),
            _ => ApiError::internal(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryability_matches_error_kind() {
        assert!(StorageError::Io(std::io::Error::other("disk hiccup")).is_retryable());
        assert!(!StorageError::InvalidData("bad row".to_string()).is_retryable());
        let json_error = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        assert!(!StorageError::Serialization(json_error).is_retryable());
    }
}

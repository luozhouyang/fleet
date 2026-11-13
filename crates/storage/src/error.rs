//! Error type shared by every storage operation.

use agent_fleet_common::ApiError;

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

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match &error {
            StorageError::InvalidData(message) => ApiError::invalid(message.clone()),
            _ => ApiError::internal(error.to_string()),
        }
    }
}

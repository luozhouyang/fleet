//! Error type returned by provider adapters and the runtime.

use agent_fleet_common::{error_codes, ApiError};
use agent_fleet_process_manager::ProcessError;

/// Errors raised by the agent runtime and its provider adapters.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The requested provider id is not registered.
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    /// The requested session id is unknown.
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// The requested permission id is unknown.
    #[error("permission not found: {0}")]
    PermissionNotFound(String),
    /// The session is not in a state that allows the operation.
    #[error("invalid session state: {0}")]
    InvalidState(String),
    /// The provider genuinely does not support the requested operation.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The request was malformed.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// An operation timed out.
    #[error("timed out: {0}")]
    Timeout(String),
    /// The provider binary could not be started or is unavailable.
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
    /// A provider reported an error (native protocol or process level).
    #[error("provider error ({code}): {message}")]
    Provider {
        /// Stable machine-readable error code.
        code: String,
        /// Human readable message.
        message: String,
    },
    /// Child process management failed.
    #[error("process error: {0}")]
    Process(#[from] ProcessError),
    /// JSON serialization or protocol parsing failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// An I/O operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Persistence failed.
    #[error("storage error: {0}")]
    Storage(#[from] agent_fleet_storage::StorageError),
    /// The operation was cancelled (for example, during shutdown).
    #[error("cancelled: {0}")]
    Cancelled(String),
}

impl RuntimeError {
    /// Build a provider error from a code and message.
    pub fn provider(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Provider {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Convert into the wire-level [`ApiError`] representation.
    pub fn to_api_error(&self) -> ApiError {
        let code = match self {
            Self::ProviderNotFound(_) | Self::ProviderUnavailable(_) => {
                error_codes::PROVIDER_UNAVAILABLE
            }
            Self::SessionNotFound(_) | Self::PermissionNotFound(_) => error_codes::NOT_FOUND,
            Self::InvalidState(_) | Self::InvalidRequest(_) => error_codes::INVALID_REQUEST,
            Self::Unsupported(_) => error_codes::UNSUPPORTED,
            Self::Timeout(_) => error_codes::TIMEOUT,
            Self::Cancelled(_) => error_codes::CONFLICT,
            Self::Provider { code, .. } => code.as_str(),
            Self::Process(_) | Self::Json(_) | Self::Io(_) | Self::Storage(_) => {
                error_codes::INTERNAL
            }
        };
        ApiError::new(code, self.to_string())
    }
}

impl From<RuntimeError> for ApiError {
    fn from(value: RuntimeError) -> Self {
        value.to_api_error()
    }
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, RuntimeError>;

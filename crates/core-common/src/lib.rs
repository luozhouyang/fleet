//! Shared low-level primitives: IDs, timestamps, errors, pagination and logging.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type MachineId = String;
pub type WorkspaceId = String;
/// Stable organization identifier.
pub type OrganizationId = String;
/// Stable user identifier.
pub type UserId = String;
pub type ProjectId = String;
pub type SessionId = String;
pub type TurnId = String;
pub type EventId = String;
pub type MessageId = String;
pub type RequestId = String;
pub type StreamId = String;

/// Timestamps are always UTC and serialized as RFC 3339 strings.
pub type Timestamp = DateTime<Utc>;

pub fn now() -> Timestamp {
    Utc::now()
}

/// Generate a prefixed, URL-safe identifier, e.g. `session_9f1c...`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// Stable, machine-readable error payload used on every API surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(error_codes::INVALID_REQUEST, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(error_codes::NOT_FOUND, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(error_codes::INTERNAL, message)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self::internal(value.to_string())
    }
}

/// Cursor-based pagination wrapper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, next_cursor: Option<String>) -> Self {
        Self { items, next_cursor }
    }
}

/// Simple acknowledgement returned by commands that produce no data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    pub ok: bool,
}

impl Ack {
    pub fn ok() -> Self {
        Self { ok: true }
    }
}

/// Well-known error codes. Adapters may add their own prefixed codes.
pub mod error_codes {
    pub const INVALID_REQUEST: &str = "invalid_request";
    pub const NOT_FOUND: &str = "not_found";
    pub const UNAUTHORIZED: &str = "unauthorized";
    pub const FORBIDDEN: &str = "forbidden";
    pub const CONFLICT: &str = "conflict";
    pub const TIMEOUT: &str = "timeout";
    pub const MACHINE_OFFLINE: &str = "machine_offline";
    pub const UNSUPPORTED: &str = "unsupported";
    pub const INTERNAL: &str = "internal";
    pub const PROVIDER_UNAVAILABLE: &str = "provider_unavailable";
    pub const PATH_DENIED: &str = "path_denied";
    pub const NOT_A_REPOSITORY: &str = "not_a_repository";
    pub const FILE_TOO_LARGE: &str = "file_too_large";
    pub const BINARY_FILE: &str = "binary_file";
    pub const PROTOCOL_ERROR: &str = "protocol_error";
    pub const PAIRING_CODE_INVALID: &str = "pairing_code_invalid";
    pub const MACHINE_NOT_PAIRED: &str = "machine_not_paired";
    pub const MACHINE_TOKEN_REQUIRED: &str = "machine_token_required";
    pub const MACHINE_KEY_MISMATCH: &str = "machine_key_mismatch";
    pub const MACHINE_ORG_MISMATCH: &str = "machine_org_mismatch";
}

/// Initialize tracing once. Safe to call from both binaries.
pub fn init_tracing(default_filter: &str) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .try_init();
}

/// Initialize tracing to an append-only file. Windows services have no
/// console, so the servers use this in service mode.
pub fn init_tracing_to_file(default_filter: &str, path: &std::path::Path) -> anyhow::Result<()> {
    use std::fs::OpenOptions;
    use std::sync::Mutex;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_writer(Mutex::new(file)),
        )
        .try_init();
    Ok(())
}

/// Process-wide shutdown requests. The Windows service control handler uses
/// this to ask the server loop to stop gracefully.
pub mod shutdown {
    use std::sync::OnceLock;

    use tokio::sync::watch;

    fn sender() -> &'static watch::Sender<bool> {
        static SENDER: OnceLock<watch::Sender<bool>> = OnceLock::new();
        SENDER.get_or_init(|| {
            let (sender, _receiver) = watch::channel(false);
            sender
        })
    }

    /// Ask every waiter to shut down.
    pub fn request() {
        sender().send_replace(true);
    }

    /// Resolve when [`request`] has been called, including calls that happened
    /// before this future was created.
    pub async fn wait() {
        let mut receiver = sender().subscribe();
        if *receiver.borrow() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_has_prefix() {
        let id = new_id("session");
        assert!(id.starts_with("session_"));
        assert!(id.len() > "session_".len());
    }

    #[test]
    fn api_error_roundtrip() {
        let err = ApiError::new("not_found", "missing").with_details(serde_json::json!({"id": 1}));
        let json = serde_json::to_string(&err).unwrap();
        let back: ApiError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, back);
        assert!(!json.contains("null"));
    }
}

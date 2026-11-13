use agent_fleet_common::{error_codes, ApiError};

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("workspace error: {0}")]
    Workspace(String),
    #[error("git error: {0}")]
    Git(String),
    #[error("agent runtime error: {0}")]
    Runtime(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl From<DaemonError> for ApiError {
    fn from(value: DaemonError) -> Self {
        let code = match &value {
            DaemonError::Invalid(_) => error_codes::INVALID_REQUEST,
            DaemonError::NotFound(_) => error_codes::NOT_FOUND,
            DaemonError::Connection(_) => error_codes::MACHINE_OFFLINE,
            _ => error_codes::INTERNAL,
        };
        ApiError::new(code, value.to_string())
    }
}

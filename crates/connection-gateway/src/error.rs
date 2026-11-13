use agent_fleet_common::{error_codes, ApiError};

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("websocket error: {0}")]
    WebSocket(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("machine offline: {0}")]
    MachineOffline(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("http error: {0}")]
    Http(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for GatewayError {
    fn from(value: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(value.to_string())
    }
}

impl From<ApiError> for GatewayError {
    fn from(value: ApiError) -> Self {
        match value.code.as_str() {
            error_codes::UNAUTHORIZED => Self::Unauthorized(value.message),
            error_codes::MACHINE_OFFLINE => Self::MachineOffline(value.message),
            _ => Self::Protocol(value.to_string()),
        }
    }
}

impl From<GatewayError> for ApiError {
    fn from(value: GatewayError) -> Self {
        let code = match &value {
            GatewayError::Unauthorized(_) | GatewayError::Auth(_) => error_codes::UNAUTHORIZED,
            GatewayError::MachineOffline(_) => error_codes::MACHINE_OFFLINE,
            GatewayError::Timeout(_) => error_codes::TIMEOUT,
            GatewayError::Protocol(_) | GatewayError::Serialization(_) => {
                error_codes::PROTOCOL_ERROR
            }
            _ => error_codes::INTERNAL,
        };
        ApiError::new(code, value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, GatewayError>;

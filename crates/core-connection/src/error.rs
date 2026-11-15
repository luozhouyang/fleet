use fleet_core_common::{error_codes, ApiError};

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
    #[error("machine revoked: {0}")]
    Revoked(String),
    #[error("machine is not paired (machine_not_paired): {0}")]
    NotPaired(String),
    #[error("machine token is required: {0}")]
    TokenRequired(String),
    #[error("pairing code is invalid: {0}")]
    PairingCodeInvalid(String),
    #[error("machine token was rejected: {0}")]
    TokenRejected(String),
    #[error("machine key mismatch: {0}")]
    MachineKeyMismatch(String),
    #[error("machine organization mismatch: {0}")]
    MachineOrgMismatch(String),
    #[error("failed to save the machine token: {0}")]
    TokenSave(String),
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
            error_codes::FORBIDDEN => Self::Revoked(value.message),
            error_codes::MACHINE_OFFLINE => Self::MachineOffline(value.message),
            error_codes::MACHINE_NOT_PAIRED => Self::NotPaired(value.message),
            error_codes::MACHINE_TOKEN_REQUIRED => Self::TokenRequired(value.message),
            error_codes::PAIRING_CODE_INVALID => Self::PairingCodeInvalid(value.message),
            error_codes::MACHINE_KEY_MISMATCH => Self::MachineKeyMismatch(value.message),
            error_codes::MACHINE_ORG_MISMATCH => Self::MachineOrgMismatch(value.message),
            _ => Self::Protocol(value.to_string()),
        }
    }
}

impl From<GatewayError> for ApiError {
    fn from(value: GatewayError) -> Self {
        let code = match &value {
            GatewayError::Unauthorized(_) | GatewayError::Auth(_) => error_codes::UNAUTHORIZED,
            GatewayError::Revoked(_) => error_codes::FORBIDDEN,
            GatewayError::NotPaired(_) => error_codes::MACHINE_NOT_PAIRED,
            GatewayError::TokenRequired(_) => error_codes::MACHINE_TOKEN_REQUIRED,
            GatewayError::PairingCodeInvalid(_) => error_codes::PAIRING_CODE_INVALID,
            GatewayError::TokenRejected(_) => error_codes::UNAUTHORIZED,
            GatewayError::MachineKeyMismatch(_) => error_codes::MACHINE_KEY_MISMATCH,
            GatewayError::MachineOrgMismatch(_) => error_codes::MACHINE_ORG_MISMATCH,
            GatewayError::TokenSave(_) => error_codes::INTERNAL,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_pairing_error_codes_to_terminal_variants() {
        let err: GatewayError = ApiError::new(error_codes::MACHINE_NOT_PAIRED, "not paired").into();
        assert!(matches!(err, GatewayError::NotPaired(_)));

        let err: GatewayError =
            ApiError::new(error_codes::MACHINE_TOKEN_REQUIRED, "token required").into();
        assert!(matches!(err, GatewayError::TokenRequired(_)));

        let err: GatewayError = ApiError::new(error_codes::PAIRING_CODE_INVALID, "bad code").into();
        assert!(matches!(err, GatewayError::PairingCodeInvalid(_)));

        let err: GatewayError = ApiError::new(error_codes::MACHINE_KEY_MISMATCH, "key").into();
        assert!(matches!(err, GatewayError::MachineKeyMismatch(_)));
        let err: GatewayError = ApiError::new(error_codes::MACHINE_ORG_MISMATCH, "org").into();
        assert!(matches!(err, GatewayError::MachineOrgMismatch(_)));
        let err: GatewayError = ApiError::new(error_codes::FORBIDDEN, "revoked").into();
        assert!(matches!(err, GatewayError::Revoked(_)));
    }
}

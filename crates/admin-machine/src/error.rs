use fleet_core_common::{error_codes, ApiError};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("machine {0} not found")]
    NotFound(String),
    #[error("machine {0} presented a public key that does not match the registered key")]
    KeyMismatch(String),
    #[error("invalid registration: {0}")]
    InvalidRegistration(String),
}

impl From<fleet_admin_storage::StorageError> for RegistryError {
    fn from(value: fleet_admin_storage::StorageError) -> Self {
        Self::Storage(value.to_string())
    }
}

impl From<RegistryError> for ApiError {
    fn from(value: RegistryError) -> Self {
        let code = match &value {
            RegistryError::NotFound(_) => error_codes::NOT_FOUND,
            RegistryError::KeyMismatch(_) => error_codes::UNAUTHORIZED,
            RegistryError::InvalidRegistration(_) => error_codes::INVALID_REQUEST,
            RegistryError::Storage(_) => error_codes::INTERNAL,
        };
        ApiError::new(code, value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, RegistryError>;

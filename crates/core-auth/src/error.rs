//! Error type shared by the authentication primitives.

use std::path::{Path, PathBuf};

use fleet_core_common::{error_codes, ApiError};
use thiserror::Error;

/// Errors produced by machine identity, signature verification and token auth.
#[derive(Debug, Error)]
pub enum AuthError {
    /// The identity file on disk is malformed or internally inconsistent.
    #[error("invalid machine key file: {0}")]
    InvalidKeyFile(String),
    /// A value expected to be base64 was not valid base64.
    #[error("invalid base64 value: {0}")]
    InvalidBase64(String),
    /// A key or signature decoded to the wrong length, or is not a valid Ed25519 key.
    #[error("invalid ed25519 key: {0}")]
    InvalidKey(String),
    /// The signature did not verify for the given data and public key.
    #[error("signature verification failed")]
    InvalidSignature,
    /// Credentials were missing, malformed, or did not match.
    #[error("unauthorized")]
    Unauthorized,
    /// No token was configured through the environment.
    #[error(
        "no token configured: set AGENT_FLEET_ADMIN_TOKEN or AGENT_FLEET_TOKEN \
         (or AGENT_FLEET_DEV=1 for the development token)"
    )]
    TokenNotConfigured,
    /// The key file is readable by group or other users.
    ///
    /// On Unix, [`crate::MachineIdentity::load`] refuses to use a key file whose
    /// mode grants any permission beyond the owner.
    #[error("machine key file {path} has insecure permissions {mode:04o}; expected 0600")]
    InsecurePermissions {
        /// Path to the offending key file.
        path: PathBuf,
        /// Observed permission bits (masked to `0o777`).
        mode: u32,
    },
    /// An I/O operation on a key file failed.
    #[error("i/o error for {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The key file could not be serialized or parsed as JSON.
    #[error("key file serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl AuthError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    fn is_internal(&self) -> bool {
        matches!(
            self,
            Self::Io { .. } | Self::Serialization(_) | Self::InsecurePermissions { .. }
        )
    }
}

/// Convenient result alias for this crate.
pub type Result<T> = std::result::Result<T, AuthError>;

impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        let code = if err.is_internal() {
            error_codes::INTERNAL
        } else {
            error_codes::UNAUTHORIZED
        };
        ApiError::new(code, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_failures_map_to_unauthorized() {
        let api: ApiError = AuthError::Unauthorized.into();
        assert_eq!(api.code, error_codes::UNAUTHORIZED);
        assert_eq!(api.message, "unauthorized");

        let api: ApiError = AuthError::TokenNotConfigured.into();
        assert_eq!(api.code, error_codes::UNAUTHORIZED);
    }

    #[test]
    fn io_failures_map_to_internal() {
        let err = AuthError::Io {
            path: PathBuf::from("/tmp/machine.key"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };
        let api: ApiError = err.into();
        assert_eq!(api.code, error_codes::INTERNAL);
        assert!(api.message.contains("/tmp/machine.key"));
    }
}

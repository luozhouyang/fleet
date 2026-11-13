//! HTTP error type shared by every control-plane route and service.
//!
//! All failures are rendered as the protocol [`ApiError`] JSON shape:
//! `{"code":"...","message":"..."}`. Internal details are logged, never sent
//! to the browser.

use agent_fleet_common::{error_codes, ApiError};
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::de::DeserializeOwned;

/// An [`ApiError`] paired with the HTTP status code used to deliver it.
#[derive(Debug, Clone)]
pub struct AppError {
    /// HTTP status returned to the caller.
    pub status: StatusCode,
    /// Stable, machine-readable error payload.
    pub error: ApiError,
}

impl AppError {
    /// Construct an error from a status, code and message.
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            error: ApiError::new(code, message),
        }
    }

    /// `400 Bad Request` with a caller-facing explanation.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            error_codes::INVALID_REQUEST,
            message,
        )
    }

    /// `401 Unauthorized`.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, error_codes::UNAUTHORIZED, message)
    }

    /// `404 Not Found`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, error_codes::NOT_FOUND, message)
    }

    /// `503 Service Unavailable` for a machine that is not connected.
    pub fn machine_offline(machine_id: &str) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::MACHINE_OFFLINE,
            format!("machine {machine_id} is offline"),
        )
    }

    /// `502 Bad Gateway` when a daemon returns an unusable payload.
    pub fn bad_gateway(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            error_codes::PROTOCOL_ERROR,
            message,
        )
    }

    /// `500 Internal Server Error`.
    ///
    /// The caller sees a generic message; the supplied detail is logged.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::error!(error = %detail, "internal control-plane error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            error_codes::INTERNAL,
            "internal server error",
        )
    }

    /// Convert a gateway/daemon error, mapping its stable code to HTTP status.
    pub fn from_api(error: ApiError) -> Self {
        let status = match error.code.as_str() {
            error_codes::NOT_FOUND => StatusCode::NOT_FOUND,
            error_codes::CONFLICT => StatusCode::CONFLICT,
            error_codes::MACHINE_OFFLINE => StatusCode::SERVICE_UNAVAILABLE,
            error_codes::TIMEOUT => StatusCode::REQUEST_TIMEOUT,
            error_codes::UNAUTHORIZED => StatusCode::UNAUTHORIZED,
            error_codes::FORBIDDEN => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_REQUEST,
        };
        if status.is_server_error() {
            tracing::error!(code = %error.code, message = %error.message, "upstream error");
        }
        Self { status, error }
    }

    /// Map a storage failure to a generic `500` while logging the real cause.
    pub fn storage(detail: impl std::fmt::Display) -> Self {
        Self::internal(format!("storage error: {detail}"))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(self.error)).into_response()
    }
}

impl From<ApiError> for AppError {
    fn from(value: ApiError) -> Self {
        Self::from_api(value)
    }
}

impl From<agent_fleet_storage::StorageError> for AppError {
    fn from(value: agent_fleet_storage::StorageError) -> Self {
        Self::storage(value)
    }
}

impl From<agent_fleet_machine_registry::RegistryError> for AppError {
    fn from(value: agent_fleet_machine_registry::RegistryError) -> Self {
        Self::from_api(value.into())
    }
}

/// JSON body extractor that renders malformed bodies as an [`ApiError`].
///
/// Axum's built-in [`Json`] rejection is plain text; the browser client always
/// expects the stable error envelope.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(AppError::bad_request(format!(
                "invalid request body: {rejection}"
            ))),
        }
    }
}

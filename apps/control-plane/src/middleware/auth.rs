//! Bearer-token authentication for the REST API.

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::state::AppState;

/// Require `Authorization: Bearer <admin_token>` on the request.
///
/// `/api/v1/health` is mounted outside this middleware, and the browser event
/// WebSocket performs its own header-or-query authentication because browser
/// WebSocket clients cannot set request headers.
pub async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    match state.auth.verify_bearer(header) {
        Ok(()) => next.run(request).await,
        Err(_) => AppError::unauthorized("missing or invalid bearer token").into_response(),
    }
}

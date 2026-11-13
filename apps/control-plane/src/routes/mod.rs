//! REST route table, mounted at `/api/v1`.

pub mod events;
pub mod git;
pub mod health;
pub mod machines;
pub mod permissions;
pub mod sessions;
pub mod workspaces;

use agent_fleet_common::ApiError;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::middleware::auth;
use crate::state::AppState;

/// Build the `/api/v1` router.
///
/// Every route except `/health` and the browser event WebSocket is wrapped in
/// the bearer-token middleware. The WebSocket authenticates itself because
/// browsers cannot attach headers to a WebSocket upgrade.
pub fn api_router(state: AppState) -> Router<AppState> {
    let protected = Router::new()
        .route("/machines", get(machines::list))
        .route(
            "/machines/{id}",
            get(machines::get).delete(machines::delete),
        )
        .route(
            "/machines/{id}/reconnect",
            axum::routing::post(machines::reconnect),
        )
        .route("/machines/{id}/providers", get(machines::providers))
        .route(
            "/workspaces",
            get(workspaces::list).post(workspaces::register),
        )
        .route(
            "/workspaces/{id}",
            get(workspaces::get).delete(workspaces::delete),
        )
        .route("/workspaces/{id}/tree", get(workspaces::tree))
        .route("/workspaces/{id}/files", get(workspaces::files))
        .route("/workspaces/{id}/search", get(workspaces::search))
        .route("/workspaces/{id}/git/status", get(git::status))
        .route("/workspaces/{id}/git/branches", get(git::branches))
        .route("/workspaces/{id}/git/log", get(git::log))
        .route("/workspaces/{id}/git/diff", get(git::diff))
        .route("/workspaces/{id}/git/diff/summary", get(git::diff_summary))
        .route("/sessions", get(sessions::list).post(sessions::create))
        .route(
            "/sessions/{id}",
            get(sessions::get).delete(sessions::delete),
        )
        .route("/sessions/{id}/messages", post(sessions::send_message))
        .route("/sessions/{id}/interrupt", post(sessions::interrupt))
        .route("/sessions/{id}/resume", post(sessions::resume))
        .route("/sessions/{id}/permissions", get(sessions::permissions))
        .route("/permissions/{id}/approve", post(permissions::approve))
        .route("/permissions/{id}/reject", post(permissions::reject))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    Router::new()
        .route("/health", get(health::health))
        .route("/events", get(events::events_ws))
        .merge(protected)
        .fallback(api_not_found)
}

/// Unmatched `/api/v1` paths return the stable JSON error envelope.
async fn api_not_found() -> impl axum::response::IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError::not_found("route not found")),
    )
}

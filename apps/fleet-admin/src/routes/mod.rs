//! REST route table, mounted at `/api/v1`.

pub mod audit;
pub mod events;
pub mod git;
pub mod health;
pub mod machines;
pub mod orgs;
pub mod pairing;
pub mod permissions;
pub mod questions;
pub mod sessions;
pub mod users;
pub mod workspaces;

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use fleet_core_common::ApiError;

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
        .route("/machines/revoked", get(machines::revoked))
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
            "/machines/{id}/unrevoke",
            axum::routing::post(machines::unrevoke),
        )
        .route(
            "/machines/{id}/org",
            axum::routing::put(machines::assign_org),
        )
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
        .route("/workspaces/{id}/git/stage", post(git::stage))
        .route("/workspaces/{id}/git/unstage", post(git::unstage))
        .route("/workspaces/{id}/git/commit", post(git::commit))
        .route("/workspaces/{id}/git/checkout", post(git::checkout))
        .route("/audit", get(audit::list))
        .route("/orgs", get(orgs::list).post(orgs::create))
        .route("/pairing/codes", post(pairing::create_code))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", axum::routing::delete(users::delete))
        .route(
            "/users/{id}/rotate-token",
            axum::routing::post(users::rotate_token),
        )
        .route("/sessions", get(sessions::list).post(sessions::create))
        .route(
            "/sessions/{id}",
            get(sessions::get).delete(sessions::delete),
        )
        .route("/sessions/{id}/close", post(sessions::close))
        .route("/sessions/{id}/messages", post(sessions::send_message))
        .route("/sessions/{id}/interrupt", post(sessions::interrupt))
        .route("/sessions/{id}/resume", post(sessions::resume))
        .route("/sessions/{id}/permissions", get(sessions::permissions))
        .route(
            "/sessions/{id}/permissions/{permission_id}/approve",
            post(permissions::approve),
        )
        .route(
            "/sessions/{id}/permissions/{permission_id}/reject",
            post(permissions::reject),
        )
        .route("/sessions/{id}/questions", get(sessions::questions))
        .route(
            "/sessions/{id}/questions/{question_id}/answer",
            post(questions::answer),
        )
        .route(
            "/sessions/{id}/questions/{question_id}/reject",
            post(questions::reject),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state,
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

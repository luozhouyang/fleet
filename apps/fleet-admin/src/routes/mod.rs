//! REST route table, mounted at `/api/v1`.

pub mod agents;
pub mod audit;
pub mod backup;
pub mod events;
pub mod git;
pub mod health;
mod idempotency;
pub mod machines;
pub mod oauth;
pub mod orgs;
pub mod pairing;
pub mod permissions;
pub mod questions;
pub mod ready;
pub mod session;
pub mod session_ui;
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
/// Every route except `/health`, `/ready` and the browser event WebSocket is
/// wrapped in the bearer-token middleware. The WebSocket authenticates itself
/// because browsers cannot attach headers to a WebSocket upgrade.
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
        .route(
            "/machines/{id}/rotate-token",
            axum::routing::post(machines::rotate_token).delete(machines::cancel_token_rotation),
        )
        .route("/machines/{id}/providers", get(machines::providers))
        .route("/agents", get(agents::list))
        .route("/agents/{id}", get(agents::get))
        .route("/agents/{id}/probe", post(agents::probe))
        .route("/drivers", get(agents::list_drivers))
        .route("/drivers/{id}", get(agents::get_driver))
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
        .route("/workspaces/{id}/files/raw", get(workspaces::raw_file))
        .route("/workspaces/{id}/search", get(workspaces::search))
        .route("/workspaces/{id}/git/status", get(git::status))
        .route("/workspaces/{id}/git/branches", get(git::branches))
        .route("/workspaces/{id}/git/log", get(git::log))
        .route("/workspaces/{id}/git/diff", get(git::diff))
        .route("/workspaces/{id}/git/diff/raw", get(git::diff_raw))
        .route("/workspaces/{id}/git/diff/summary", get(git::diff_summary))
        .route("/workspaces/{id}/git/stage", post(git::stage))
        .route("/workspaces/{id}/git/unstage", post(git::unstage))
        .route("/workspaces/{id}/git/commit", post(git::commit))
        .route("/workspaces/{id}/git/checkout", post(git::checkout))
        .route("/audit", get(audit::list))
        .route("/orgs", get(orgs::list).post(orgs::create))
        .route(
            "/orgs/{org_id}",
            get(orgs::get).patch(orgs::rename).delete(orgs::delete),
        )
        .route(
            "/orgs/{org_id}/members",
            get(orgs::members).post(orgs::add_member),
        )
        .route(
            "/orgs/{org_id}/members/{user_id}",
            axum::routing::patch(orgs::update_member).delete(orgs::remove_member),
        )
        .route("/orgs/{org_id}/leave", post(orgs::leave))
        .route(
            "/orgs/{org_id}/invitations",
            get(orgs::invitations).post(orgs::create_invitation),
        )
        .route(
            "/orgs/{org_id}/invitations/{invitation_id}",
            axum::routing::delete(orgs::revoke_invitation),
        )
        .route("/admin/unowned-machines", get(orgs::unowned_machines))
        .route("/admin/claim-machine", post(orgs::claim_machine))
        .route("/admin/backup", get(backup::download))
        .route("/pairing/codes", post(pairing::create_code))
        .route("/auth/session", post(session::create_session))
        .route("/me", get(users::me))
        .route("/me/identities", get(users::identities))
        .route(
            "/me/identities/{id}",
            axum::routing::delete(users::unlink_identity),
        )
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", axum::routing::delete(users::delete))
        .route("/users/{id}/disable", post(users::disable))
        .route("/users/{id}/enable", post(users::enable))
        .route(
            "/users/{id}/rotate-token",
            axum::routing::post(users::rotate_token),
        )
        .route("/sessions", get(sessions::list).post(sessions::create))
        .route(
            "/sessions/ui",
            axum::routing::patch(session_ui::update_ui_bulk),
        )
        .route("/sessions/read", post(session_ui::mark_read))
        .route(
            "/sessions/pinned-order",
            axum::routing::put(session_ui::pinned_order),
        )
        .route("/sessions/facets", get(sessions::facets))
        .route(
            "/sessions/{id}/ui",
            axum::routing::patch(session_ui::update_ui),
        )
        .route("/sessions/{id}/read", post(session_ui::mark_one_read))
        .route("/sessions/{id}/unread", post(session_ui::mark_one_unread))
        .route(
            "/session-folders",
            get(session_ui::list_folders).post(session_ui::create_folder),
        )
        .route(
            "/session-folders/{id}",
            axum::routing::patch(session_ui::rename_folder).delete(session_ui::delete_folder),
        )
        .route(
            "/sessions/{id}",
            get(sessions::get)
                .patch(sessions::update)
                .delete(sessions::delete),
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
        .route("/ready", get(ready::ready))
        .route("/events", get(events::events_ws))
        .route(
            "/realtime/ticket",
            axum::routing::post(events::create_ticket),
        )
        .route("/auth/logout", post(session::logout))
        .route("/auth/providers", get(oauth::providers))
        .route("/auth/login/{provider}", get(oauth::login))
        .route("/auth/callback/{provider}", get(oauth::callback))
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

//! Agent session endpoints.

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use fleet_core_protocol::agent::{
    AgentPermissionRequest, AgentQuestionRequest, AgentSession, SendMessageRequest,
    UpdateSessionRequest,
};
use fleet_core_protocol::rpc::{CreateSessionRpcRequest, SessionDetail};

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::services::agent_service::AgentService;
use crate::state::AppState;
use serde::Deserialize;

/// Query for `GET /sessions`.
#[derive(Debug, Deserialize, Default)]
pub struct ListSessionsQuery {
    /// Include archived (`terminated`) sessions.
    #[serde(default)]
    pub include_closed: Option<bool>,
}

/// `GET /api/v1/sessions`
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<AgentSession>>, AppError> {
    let mut sessions =
        AgentService::list_sessions(&state, query.include_closed.unwrap_or(false)).await?;
    if let Some(visible) = acl::visible_machine_ids(&state, &principal).await? {
        sessions.retain(|session| visible.contains(&session.machine_id));
    }
    Ok(Json(sessions))
}

/// `POST /api/v1/sessions`
pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateSessionRpcRequest>,
) -> Result<Json<AgentSession>, AppError> {
    acl::ensure_machine_access(&state, &principal, &request.machine_id).await?;
    if let Some(workspace_id) = request.create.workspace_id.as_deref() {
        acl::ensure_workspace_access(&state, &principal, workspace_id).await?;
    }
    Ok(Json(AgentService::create_session(&state, request).await?))
}

/// `GET /api/v1/sessions/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SessionDetail>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::get_session(&state, &id).await?))
}

/// `PATCH /api/v1/sessions/{id}` — update mutable session metadata.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<UpdateSessionRequest>,
) -> Result<Json<AgentSession>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(
        AgentService::update_session(&state, &id, request).await?,
    ))
}

/// `POST /api/v1/sessions/{id}/close`
pub async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::close(&state, &id).await?))
}

/// `DELETE /api/v1/sessions/{id}` — permanently removes history.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::delete(&state, &id).await?))
}

/// `POST /api/v1/sessions/{id}/messages`
pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(message): ApiJson<SendMessageRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(
        AgentService::send_message(&state, &id, message).await?,
    ))
}

/// `POST /api/v1/sessions/{id}/interrupt`
pub async fn interrupt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::interrupt(&state, &id).await?))
}

/// `POST /api/v1/sessions/{id}/resume`
pub async fn resume(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::resume(&state, &id).await?))
}

/// `GET /api/v1/sessions/{id}/permissions`
pub async fn permissions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<AgentPermissionRequest>>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::list_permissions(&state, &id).await?))
}

pub async fn questions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<AgentQuestionRequest>>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::list_questions(&state, &id).await?))
}

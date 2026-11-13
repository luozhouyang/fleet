//! Agent session endpoints.

use agent_fleet_protocol::agent::{AgentPermissionRequest, AgentSession, SendMessageRequest};
use agent_fleet_protocol::rpc::{CreateSessionRpcRequest, SessionDetail};
use axum::extract::{Path, State};
use axum::Json;

use crate::error::{ApiJson, AppError};
use crate::services::agent_service::AgentService;
use crate::state::AppState;

/// `GET /api/v1/sessions`
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<AgentSession>>, AppError> {
    Ok(Json(AgentService::list_sessions(&state).await?))
}

/// `POST /api/v1/sessions`
pub async fn create(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<CreateSessionRpcRequest>,
) -> Result<Json<AgentSession>, AppError> {
    Ok(Json(AgentService::create_session(&state, request).await?))
}

/// `GET /api/v1/sessions/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionDetail>, AppError> {
    Ok(Json(AgentService::get_session(&state, &id).await?))
}

/// `DELETE /api/v1/sessions/{id}`
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(AgentService::shutdown(&state, &id).await?))
}

/// `POST /api/v1/sessions/{id}/messages`
pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(message): ApiJson<SendMessageRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        AgentService::send_message(&state, &id, message).await?,
    ))
}

/// `POST /api/v1/sessions/{id}/interrupt`
pub async fn interrupt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(AgentService::interrupt(&state, &id).await?))
}

/// `POST /api/v1/sessions/{id}/resume`
pub async fn resume(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(AgentService::resume(&state, &id).await?))
}

/// `GET /api/v1/sessions/{id}/permissions`
pub async fn permissions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<AgentPermissionRequest>>, AppError> {
    Ok(Json(AgentService::list_permissions(&state, &id).await?))
}

//! Permission approval endpoints.

use agent_fleet_protocol::rpc::RespondPermissionHttpRequest;
use axum::extract::{Path, State};
use axum::Json;

use crate::error::AppError;
use crate::services::agent_service::AgentService;
use crate::state::AppState;

/// `POST /api/v1/permissions/{id}/approve`
pub async fn approve(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<RespondPermissionHttpRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let message = body.and_then(|Json(body)| body.message);
    Ok(Json(
        AgentService::respond_permission(&state, &id, true, message).await?,
    ))
}

/// `POST /api/v1/permissions/{id}/reject`
pub async fn reject(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<RespondPermissionHttpRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let message = body.and_then(|Json(body)| body.message);
    Ok(Json(
        AgentService::respond_permission(&state, &id, false, message).await?,
    ))
}

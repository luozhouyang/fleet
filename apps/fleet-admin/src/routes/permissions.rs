//! Permission approval endpoints.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Extension;
use axum::Json;
use fleet_core_protocol::rpc::RespondPermissionHttpRequest;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::routes::idempotency::idempotency_key;
use crate::services::acl;
use crate::services::agent_service::AgentService;
use crate::state::AppState;

/// `POST /api/v1/sessions/{session_id}/permissions/{permission_id}/approve`
pub async fn approve(
    State(state): State<AppState>,
    Path((session_id, permission_id)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Option<Json<RespondPermissionHttpRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = idempotency_key(&headers)?;
    acl::ensure_session_access(&state, &principal, &session_id).await?;
    let (option_id, message) = body
        .map(|Json(body)| (body.option_id, body.message))
        .unwrap_or((None, None));
    Ok(Json(
        AgentService::respond_permission(
            &state,
            &session_id,
            &permission_id,
            true,
            option_id,
            message,
            key.as_deref(),
        )
        .await?,
    ))
}

/// `POST /api/v1/sessions/{session_id}/permissions/{permission_id}/reject`
pub async fn reject(
    State(state): State<AppState>,
    Path((session_id, permission_id)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: Option<Json<RespondPermissionHttpRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = idempotency_key(&headers)?;
    acl::ensure_session_access(&state, &principal, &session_id).await?;
    let (option_id, message) = body
        .map(|Json(body)| (body.option_id, body.message))
        .unwrap_or((None, None));
    Ok(Json(
        AgentService::respond_permission(
            &state,
            &session_id,
            &permission_id,
            false,
            option_id,
            message,
            key.as_deref(),
        )
        .await?,
    ))
}

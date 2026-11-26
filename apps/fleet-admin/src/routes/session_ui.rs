//! Per-user session presentation endpoints: pins, archive, labels, folders and
//! read markers.
//!
//! These routes only touch admin-owned metadata, so they never require the
//! machine to be online. Each one resolves the calling actor and enforces the
//! same machine ACL as the rest of the session API.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use fleet_core_protocol::agent::{
    AgentSession, BulkUpdateSessionUiRequest, UpdateSessionUiRequest,
};
use serde::Deserialize;

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::services::session_ui_service::{self, actor_id};
use crate::state::AppState;

/// `PATCH /api/v1/sessions/{id}/ui`
pub async fn update_ui(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<UpdateSessionUiRequest>,
) -> Result<Json<AgentSession>, AppError> {
    let session = acl::ensure_session_access(&state, &principal, &id).await?;
    let actor = actor_id(&principal);
    let patch = session_ui_service::to_patch(&state, &actor, &request).await?;
    Ok(Json(
        session_ui_service::patch_session(&state, &actor, session, &patch).await?,
    ))
}

/// `PATCH /api/v1/sessions/ui` — apply one patch to several sessions.
pub async fn update_ui_bulk(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<BulkUpdateSessionUiRequest>,
) -> Result<StatusCode, AppError> {
    if request.ids.is_empty() {
        return Err(AppError::bad_request("ids must not be empty"));
    }
    if request.ids.len() > session_ui_service::MAX_BULK_IDS {
        return Err(AppError::bad_request(format!(
            "at most {} sessions can be updated at once",
            session_ui_service::MAX_BULK_IDS
        )));
    }
    for id in &request.ids {
        acl::ensure_session_access(&state, &principal, id).await?;
    }
    let actor = actor_id(&principal);
    let patch = session_ui_service::to_patch(&state, &actor, &request.patch).await?;
    session_ui_service::patch_many(&state, &actor, &request.ids, &patch).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct MarkReadRequest {
    /// The session's `updated_at` the client observed.
    pub watermark: fleet_core_common::Timestamp,
    #[serde(default)]
    pub ids: Vec<String>,
}

/// `POST /api/v1/sessions/read`
pub async fn mark_read(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<MarkReadRequest>,
) -> Result<StatusCode, AppError> {
    if request.ids.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    for id in &request.ids {
        acl::ensure_session_access(&state, &principal, id).await?;
    }
    let actor = actor_id(&principal);
    session_ui_service::mark_read(&state, &actor, &request.ids, request.watermark).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/read`
pub async fn mark_one_read(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<MarkReadRequest>,
) -> Result<StatusCode, AppError> {
    let session = acl::ensure_session_access(&state, &principal, &id).await?;
    let actor = actor_id(&principal);
    // The client's observation may be older than the stored row (a refresh can
    // race an event); the stored value is the safe upper bound because storage
    // keeps the maximum watermark.
    let watermark = request.watermark.max(session.updated_at);
    session_ui_service::mark_read(&state, &actor, &[id], watermark).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/unread`
pub async fn mark_one_unread(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    let actor = actor_id(&principal);
    session_ui_service::mark_unread(&state, &actor, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct PinnedOrderRequest {
    pub ids: Vec<String>,
}

/// `PUT /api/v1/sessions/pinned-order`
pub async fn pinned_order(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<PinnedOrderRequest>,
) -> Result<StatusCode, AppError> {
    if request.ids.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    if request.ids.len() > session_ui_service::MAX_BULK_IDS {
        return Err(AppError::bad_request(format!(
            "at most {} sessions can be reordered at once",
            session_ui_service::MAX_BULK_IDS
        )));
    }
    for id in &request.ids {
        acl::ensure_session_access(&state, &principal, id).await?;
    }
    let actor = actor_id(&principal);
    state
        .db
        .session_ui
        .replace_pins(&actor, &request.ids)
        .await
        .map_err(AppError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
}

/// `GET /api/v1/session-folders`
pub async fn list_folders(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    let actor = actor_id(&principal);
    let folders = state
        .db
        .session_ui
        .list_folders(&actor)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(serde_json::json!(folders)))
}

/// `POST /api/v1/session-folders`
pub async fn create_folder(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateFolderRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let actor = actor_id(&principal);
    let folder = state
        .db
        .session_ui
        .create_folder(&actor, &request.name)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(serde_json::json!(folder)))
}

/// `PATCH /api/v1/session-folders/{id}`
pub async fn rename_folder(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateFolderRequest>,
) -> Result<StatusCode, AppError> {
    let actor = actor_id(&principal);
    let renamed = state
        .db
        .session_ui
        .rename_folder(&actor, &id, &request.name)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if renamed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::not_found(format!("folder {id} not found")))
    }
}

/// `DELETE /api/v1/session-folders/{id}`
pub async fn delete_folder(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    let actor = actor_id(&principal);
    let deleted = state
        .db
        .session_ui
        .delete_folder(&actor, &id)
        .await
        .map_err(AppError::internal)?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::not_found(format!("folder {id} not found")))
    }
}

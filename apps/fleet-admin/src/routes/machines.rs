//! Machine endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use fleet_core_protocol::agent::ProviderInfo;
use fleet_core_protocol::authz::AssignMachineRequest;
use fleet_core_protocol::machine::{Machine, RevokedMachine};

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::services::machine_service::MachineService;
use crate::state::AppState;

/// `GET /api/v1/machines/revoked`
pub async fn revoked(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<RevokedMachine>>, AppError> {
    acl::ensure_admin(&principal)?;
    Ok(Json(MachineService::revoked(&state).await?))
}

/// `POST /api/v1/machines/{id}/unrevoke`
pub async fn unrevoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_admin(&principal)?;
    MachineService::unrevoke(&state, &id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// `GET /api/v1/machines`
pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<Machine>>, AppError> {
    let mut machines = MachineService::list(&state).await?;
    if let Some(visible) = acl::visible_machine_ids(&state, &principal).await? {
        machines.retain(|machine| visible.contains(&machine.id));
    }
    Ok(Json(machines))
}

/// `GET /api/v1/machines/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Machine>, AppError> {
    acl::ensure_machine_access(&state, &principal, &id).await?;
    Ok(Json(MachineService::get(&state, &id).await?))
}

/// `DELETE /api/v1/machines/{id}`
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    acl::ensure_machine_admin(&state, &principal, &id).await?;
    MachineService::remove(&state, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/machines/{id}/reconnect`
///
/// Drops the live connection; the daemon reconnects with its normal backoff.
pub async fn reconnect(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_machine_access(&state, &principal, &id).await?;
    let machine = MachineService::get(&state, &id).await?;
    state.connections.disconnect(&machine.id);
    Ok(Json(serde_json::json!({"ok": true})))
}

/// `GET /api/v1/machines/{id}/providers`
pub async fn providers(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ProviderInfo>>, AppError> {
    acl::ensure_machine_access(&state, &principal, &id).await?;
    Ok(Json(MachineService::providers(&state, &id).await?))
}

/// `PUT /api/v1/machines/{id}/org`
///
/// Moves a machine between organizations. Only administrators can move
/// machines, and organization admins may only assign machines they own to
/// their own organization. The legacy admin token can assign any machine to
/// any existing organization.
pub async fn assign_org(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<AssignMachineRequest>,
) -> Result<Json<Machine>, AppError> {
    acl::ensure_machine_admin(&state, &principal, &id).await?;
    let organization = state
        .db
        .organizations
        .get(&request.org_id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("organization {} not found", request.org_id)))?;
    if !principal.can_access_org(Some(&organization.id)) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            fleet_core_common::error_codes::FORBIDDEN,
            "cannot assign a machine outside your organization",
        ));
    }
    state.db.machine_orgs.assign(&id, &organization.id).await?;
    Ok(Json(MachineService::get(&state, &id).await?))
}

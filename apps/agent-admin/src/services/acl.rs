//! Organization-scoped access control helpers.
//!
//! Every resource inherits its organization from the machine that owns it:
//! workspace → machine → organization, session → machine, permission → session.
//! Unauthorized lookups return 404 so callers cannot probe other organizations.

use std::collections::HashSet;

use agent_fleet_protocol::agent::AgentSession;
use agent_fleet_protocol::workspace::Workspace;
use axum::http::StatusCode;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::workspace_service::WorkspaceService;
use crate::state::AppState;

/// Machine ids visible to the principal, or `None` for the legacy super-admin.
pub async fn visible_machine_ids(
    state: &AppState,
    principal: &Principal,
) -> Result<Option<HashSet<String>>, AppError> {
    if principal.is_legacy_admin() {
        return Ok(None);
    }
    let org = principal.org_id().unwrap_or_default();
    Ok(Some(
        state
            .db
            .machine_ids_for_org(org)
            .await?
            .into_iter()
            .collect(),
    ))
}

/// Require access to a machine.
pub async fn ensure_machine_access(
    state: &AppState,
    principal: &Principal,
    machine_id: &str,
) -> Result<(), AppError> {
    if principal.is_legacy_admin() {
        return Ok(());
    }
    let org = state.db.machine_org(machine_id).await?;
    if principal.can_access_org(org.as_deref()) {
        Ok(())
    } else {
        Err(AppError::not_found(format!(
            "machine {machine_id} not found"
        )))
    }
}

/// Require an administrator role plus access to the machine.
pub async fn ensure_machine_admin(
    state: &AppState,
    principal: &Principal,
    machine_id: &str,
) -> Result<(), AppError> {
    ensure_admin(principal)?;
    ensure_machine_access(state, principal, machine_id).await
}

/// Require an administrator role.
pub fn ensure_admin(principal: &Principal) -> Result<(), AppError> {
    if principal.is_admin() {
        Ok(())
    } else {
        Err(AppError::new(
            StatusCode::FORBIDDEN,
            agent_fleet_common::error_codes::FORBIDDEN,
            "administrator role required",
        ))
    }
}

/// Require access to a workspace and return it.
pub async fn ensure_workspace_access(
    state: &AppState,
    principal: &Principal,
    workspace_id: &str,
) -> Result<Workspace, AppError> {
    let workspace = WorkspaceService::get(state, workspace_id).await?;
    ensure_machine_access(state, principal, &workspace.machine_id).await?;
    Ok(workspace)
}

/// Require access to a session and return it.
pub async fn ensure_session_access(
    state: &AppState,
    principal: &Principal,
    session_id: &str,
) -> Result<AgentSession, AppError> {
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("session {session_id} not found")))?;
    ensure_machine_access(state, principal, &session.machine_id).await?;
    Ok(session)
}

/// Require access to a permission request via its session.
pub async fn ensure_permission_access(
    state: &AppState,
    principal: &Principal,
    permission_id: &str,
) -> Result<(), AppError> {
    if principal.is_legacy_admin() {
        return Ok(());
    }
    let session_id = match state.db.get_permission(permission_id).await? {
        Some(permission) => Some(permission.session_id),
        None => state
            .event_index
            .permission_route(permission_id)
            .map(|route| route.session_id),
    };
    let Some(session_id) = session_id else {
        return Err(AppError::not_found(format!(
            "permission {permission_id} not found"
        )));
    };
    ensure_session_access(state, principal, &session_id).await?;
    Ok(())
}

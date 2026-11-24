#![allow(clippy::result_large_err)]

//! Organization-scoped access control helpers.
//!
//! Every resource inherits its organization from the machine that owns it:
//! workspace → machine → organization, session → machine, permission → session.
//! Unauthorized lookups return 404 so callers cannot probe other organizations.
//!
//! Authorization itself is a capability table evaluated in [`policy`]. The
//! table is deliberately a data structure rather than `if` chains so the
//! evaluation can later be replaced by an external policy engine (Casbin) with
//! the same request shape: `(role, organization, object, action)`.

use std::collections::HashSet;

use fleet_core_protocol::agent::AgentSession;
use fleet_core_protocol::authz::UserRole;
use fleet_core_protocol::workspace::Workspace;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::policy::{Action, Policy};
use crate::services::workspace_service::WorkspaceService;
use crate::state::AppState;

/// Machine ids visible to the principal in its active organization.
///
/// The legacy bootstrap admin sees every machine (`None`).
pub async fn visible_machine_ids(
    state: &AppState,
    principal: &Principal,
) -> Result<Option<HashSet<String>>, AppError> {
    if principal.is_legacy_admin() {
        return Ok(None);
    }
    let org = principal
        .org_id()
        .ok_or_else(|| AppError::forbidden("no active organization"))?;
    Ok(Some(
        state
            .db
            .machine_orgs
            .ids_for_org(org)
            .await?
            .into_iter()
            .collect(),
    ))
}

/// Organization that owns a machine, failing closed when it is unowned.
pub async fn machine_org(state: &AppState, machine_id: &str) -> Result<Option<String>, AppError> {
    Ok(state.db.machine_orgs.get(machine_id).await?)
}

/// Require the principal's role to allow `action` in its active organization.
pub fn ensure_action(principal: &Principal, action: Action) -> Result<(), AppError> {
    if Policy::allows(principal.role, action) {
        Ok(())
    } else {
        Err(AppError::forbidden(format!(
            "role '{}' may not {}",
            principal.role.as_str(),
            action.description()
        )))
    }
}

/// Require a role that may manage members and machines.
pub fn ensure_admin(principal: &Principal) -> Result<(), AppError> {
    ensure_action(principal, Action::ManageMembers)
}

/// Require organization ownership.
pub fn ensure_owner(principal: &Principal) -> Result<(), AppError> {
    ensure_action(principal, Action::ManageOrganization)
}

/// Require the principal to be a member of `org_id`, returning its role there.
pub fn ensure_org_member(principal: &Principal, org_id: &str) -> Result<UserRole, AppError> {
    if principal.is_legacy_admin() {
        return Ok(UserRole::Owner);
    }
    principal
        .role_in(org_id)
        .ok_or_else(|| AppError::not_found(format!("organization {org_id} not found")))
}

/// Require access to a machine and return the organization that owns it.
pub async fn ensure_machine_access(
    state: &AppState,
    principal: &Principal,
    machine_id: &str,
) -> Result<Option<String>, AppError> {
    if principal.is_legacy_admin() {
        return machine_org(state, machine_id).await;
    }
    let org = machine_org(state, machine_id).await?;
    match org {
        Some(org_id) if principal.role_in(&org_id).is_some() => {
            ensure_action(principal, Action::UseMachine)?;
            Ok(Some(org_id))
        }
        _ => Err(AppError::not_found(format!(
            "machine {machine_id} not found"
        ))),
    }
}

/// Require an administrator role plus access to the machine.
pub async fn ensure_machine_admin(
    state: &AppState,
    principal: &Principal,
    machine_id: &str,
) -> Result<Option<String>, AppError> {
    ensure_action(principal, Action::ManageMachines)?;
    ensure_machine_access(state, principal, machine_id).await
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
        .sessions
        .get(session_id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("session {session_id} not found")))?;
    ensure_machine_access(state, principal, &session.machine_id).await?;
    Ok(session)
}

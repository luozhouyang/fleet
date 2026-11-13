//! Audit-log listing for privileged operations.

use std::collections::HashSet;

use agent_fleet_protocol::audit::AuditLogPage;
use axum::extract::{Query, State};
use axum::Extension;
use axum::Json;
use serde::Deserialize;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::state::AppState;

/// Query for `GET /api/v1/audit`.
#[derive(Debug, Deserialize, Default)]
pub struct AuditQuery {
    /// Maximum entries to return (default 100, clamped to 1–500).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `GET /api/v1/audit` – newest privileged actions first.
///
/// Administrators see entries for their organization; the legacy admin token
/// sees everything.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<AuditLogPage>, AppError> {
    acl::ensure_admin(&principal)?;
    let mut entries = state.db.list_audit_logs(query.limit.unwrap_or(100)).await?;

    if let Some(org_id) = principal.org_id() {
        let machine_ids: HashSet<String> = state
            .db
            .machine_ids_for_org(org_id)
            .await?
            .into_iter()
            .collect();
        let mut subjects: HashSet<String> = state
            .db
            .list_workspaces(None)
            .await?
            .into_iter()
            .filter(|workspace| machine_ids.contains(&workspace.machine_id))
            .map(|workspace| workspace.id)
            .collect();
        subjects.extend(
            state
                .db
                .list_users(Some(org_id))
                .await?
                .into_iter()
                .map(|user| user.id),
        );
        entries.retain(|entry| subjects.contains(&entry.subject));
    }

    Ok(Json(AuditLogPage { entries }))
}

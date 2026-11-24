//! Audit-log listing for privileged operations.

use axum::extract::{Query, State};
use axum::Extension;
use axum::Json;
use fleet_core_protocol::audit::AuditLogPage;
use serde::Deserialize;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::services::policy::Action;
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
    acl::ensure_action(&principal, Action::ReadAudit)?;
    // Filtering happens in SQL before LIMIT so another organization's newer
    // entries cannot crowd out the caller's history.
    let entries = state
        .db
        .audit
        .list(principal.org_id(), query.limit.unwrap_or(100))
        .await?;
    Ok(Json(AuditLogPage { entries }))
}

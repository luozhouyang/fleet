//! Audit-log listing for privileged operations.

use agent_fleet_protocol::audit::AuditLogPage;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::AppError;
use crate::state::AppState;

/// Query for `GET /api/v1/audit`.
#[derive(Debug, Deserialize, Default)]
pub struct AuditQuery {
    /// Maximum entries to return (default 100, clamped to 1–500).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `GET /api/v1/audit` – newest privileged actions first.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<AuditLogPage>, AppError> {
    let entries = state.db.list_audit_logs(query.limit.unwrap_or(100)).await?;
    Ok(Json(AuditLogPage { entries }))
}

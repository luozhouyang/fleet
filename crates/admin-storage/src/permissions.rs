//! Permissions storage operations.

use fleet_core_common::now;
use fleet_core_protocol::agent::{AgentPermissionRequest, PermissionStatus};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::rows::PermissionRow;
use crate::Result;

/// Handle for permissions queries.
#[derive(Clone)]
pub struct Permissions {
    pool: SqlitePool,
}

impl Permissions {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Permissions {
    /// Insert a permission request or replace its mutable fields.
    pub async fn upsert(&self, p: &AgentPermissionRequest) -> Result<()> {
        let input_json = serde_json::to_string(&p.input)?;

        sqlx::query(
            r#"
            INSERT INTO permission_requests (
                id, session_id, turn_id, tool_call_id, tool_name, description,
                input_json, status, created_at, resolved_at, expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(id) DO UPDATE SET
                session_id = excluded.session_id,
                turn_id = excluded.turn_id,
                tool_call_id = excluded.tool_call_id,
                tool_name = excluded.tool_name,
                description = excluded.description,
                input_json = excluded.input_json,
                status = excluded.status,
                resolved_at = excluded.resolved_at,
                expires_at = excluded.expires_at
            "#,
        )
        .bind(&p.id)
        .bind(&p.session_id)
        .bind(&p.turn_id)
        .bind(&p.tool_call_id)
        .bind(&p.tool_name)
        .bind(&p.description)
        .bind(input_json)
        .bind(enum_text(&p.status))
        .bind(format_ts(p.created_at))
        .bind(format_opt_ts(p.resolved_at))
        .bind(format_opt_ts(p.expires_at))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch one permission request by id.
    pub async fn get(&self, id: &str) -> Result<Option<AgentPermissionRequest>> {
        let row = sqlx::query_as::<_, PermissionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, tool_name, description,
                   input_json, status, created_at, resolved_at, expires_at
            FROM permission_requests
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(PermissionRow::into_permission))
    }

    /// List every permission request for a session, newest first.
    pub async fn list(&self, session_id: &str) -> Result<Vec<AgentPermissionRequest>> {
        let rows = sqlx::query_as::<_, PermissionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, tool_name, description,
                   input_json, status, created_at, resolved_at, expires_at
            FROM permission_requests
            WHERE session_id = ?1
            ORDER BY created_at DESC, id DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(PermissionRow::into_permission)
            .collect())
    }

    /// List only pending permission requests for a session, oldest first.
    pub async fn list_pending(&self, session_id: &str) -> Result<Vec<AgentPermissionRequest>> {
        let rows = sqlx::query_as::<_, PermissionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, tool_name, description,
                   input_json, status, created_at, resolved_at, expires_at
            FROM permission_requests
            WHERE session_id = ?1 AND status = ?2
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .bind(enum_text(&PermissionStatus::Pending))
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(PermissionRow::into_permission)
            .collect())
    }

    /// Resolve a pending permission. Returns false when it was not pending
    /// (already resolved or missing), which makes concurrent approvals safe.
    pub async fn resolve_pending(&self, id: &str, status: PermissionStatus) -> Result<bool> {
        let resolved_at = now();
        let result = sqlx::query(
            "UPDATE permission_requests SET status = ?2, resolved_at = ?3 \
             WHERE id = ?1 AND status = 'pending'",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(format_ts(resolved_at))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Best-effort revert of a permission that failed to reach its daemon.
    pub async fn revert_pending(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE permission_requests SET status = 'pending', resolved_at = NULL \
             WHERE id = ?1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

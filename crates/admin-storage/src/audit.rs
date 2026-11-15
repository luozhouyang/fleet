//! Audit storage operations.

use chrono::{DateTime, Utc};
use fleet_core_common::now;
use fleet_core_protocol::audit::AuditLogEntry;
use serde_json::Value;
use sqlx::{FromRow, SqlitePool};

use crate::helpers::*;
use crate::Result;

/// Handle for audit queries.
#[derive(Clone)]
pub struct Audit {
    pool: SqlitePool,
}

impl Audit {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Audit {
    /// Append one audit log record.
    pub async fn append(
        &self,
        actor: &str,
        org_id: Option<&str>,
        action: &str,
        subject: &str,
        details: serde_json::Value,
    ) -> Result<()> {
        let details_json = serde_json::to_string(&details)?;

        sqlx::query(
            r#"
            INSERT INTO audit_logs (actor, org_id, action, subject, details_json, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(actor)
        .bind(org_id)
        .bind(action)
        .bind(subject)
        .bind(details_json)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// List audit records newest first, clamped to 1–500 entries.
    ///
    /// `org_id = None` returns every organization and is reserved for the
    /// legacy super-admin token.
    pub async fn list(&self, org_id: Option<&str>, limit: usize) -> Result<Vec<AuditLogEntry>> {
        let limit = limit.clamp(1, 500) as i64;
        let rows: Vec<AuditRow> = sqlx::query_as(
            r#"
            SELECT id, actor, action, subject, details_json, created_at
            FROM audit_logs
            WHERE ?1 IS NULL OR org_id = ?1
            ORDER BY id DESC
            LIMIT ?2
            "#,
        )
        .bind(org_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| AuditLogEntry {
                id: row.id,
                actor: row.actor,
                action: row.action,
                subject: row.subject,
                details: serde_json::from_str(&row.details_json).unwrap_or(Value::Null),
                created_at: DateTime::parse_from_rfc3339(&row.created_at)
                    .map(|value| value.with_timezone(&Utc))
                    .unwrap_or_else(|_| now()),
            })
            .collect())
    }
}

#[derive(Debug, FromRow)]
struct AuditRow {
    id: i64,
    actor: String,
    action: String,
    subject: String,
    details_json: String,
    created_at: String,
}

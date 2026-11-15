//! Pairing-code storage.

use fleet_core_common::{new_id, now, Timestamp};
use fleet_core_protocol::pairing::{PairingCodeInfo, PairingCodeStatus};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

/// Row shape for the `lookup` query.
#[derive(sqlx::FromRow)]
struct PairingCodeRow {
    id: String,
    org_id: String,
    created_by: String,
    machine_name_hint: Option<String>,
    expires_at: String,
    used_at: Option<String>,
    attempts: i64,
}

/// Handle for pairing-code queries.
#[derive(Clone)]
pub struct PairingCodes {
    pool: SqlitePool,
}

impl PairingCodes {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl PairingCodes {
    /// Insert a code hash and return the new id.
    pub async fn create(
        &self,
        org_id: &str,
        created_by: &str,
        machine_name_hint: Option<&str>,
        expires_at: Timestamp,
        code_hash: &str,
    ) -> Result<String> {
        let id = new_id("pair");
        sqlx::query(
            r#"
            INSERT INTO pairing_codes
                (id, code_hash, org_id, created_by, machine_name_hint, expires_at, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&id)
        .bind(code_hash)
        .bind(org_id)
        .bind(created_by)
        .bind(machine_name_hint)
        .bind(format_ts(expires_at))
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Insert a code hash and its `pairing.code.create` audit row in one
    /// transaction. A crash can therefore never leave a code without the audit
    /// record (or an audit record without the code).
    pub async fn create_audited(
        &self,
        org_id: &str,
        created_by: &str,
        machine_name_hint: Option<&str>,
        expires_at: Timestamp,
        code_hash: &str,
    ) -> Result<String> {
        let id = new_id("pair");
        let created_at = now();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            r#"
            INSERT INTO pairing_codes
                (id, code_hash, org_id, created_by, machine_name_hint, expires_at, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&id)
        .bind(code_hash)
        .bind(org_id)
        .bind(created_by)
        .bind(machine_name_hint)
        .bind(format_ts(expires_at))
        .bind(format_ts(created_at))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (actor, org_id, action, subject, details_json, created_at)
            VALUES (?1, ?2, 'pairing.code.create', ?3, ?4, ?5)
            "#,
        )
        .bind(created_by)
        .bind(org_id)
        .bind(&id)
        .bind(
            serde_json::json!({
                "code_id": id,
                "org_id": org_id,
                "expires_at": format_ts(expires_at),
                "machine_name_hint": machine_name_hint,
            })
            .to_string(),
        )
        .bind(format_ts(created_at))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Validate a code hash. Non-`Valid` statuses are never exposed to clients.
    pub async fn lookup(&self, code_hash: &str, max_attempts: u32) -> Result<PairingCodeStatus> {
        let row: Option<PairingCodeRow> = sqlx::query_as(
            r#"
                SELECT id, org_id, created_by, machine_name_hint,
                       expires_at, used_at, attempts
                FROM pairing_codes
                WHERE code_hash = ?1
                "#,
        )
        .bind(code_hash)
        .fetch_optional(&self.pool)
        .await?;

        let Some(PairingCodeRow {
            id,
            org_id,
            created_by,
            machine_name_hint,
            expires_at,
            used_at,
            attempts,
        }) = row
        else {
            return Ok(PairingCodeStatus::Invalid);
        };

        if used_at.is_some() {
            return Ok(PairingCodeStatus::Used { id, org_id });
        }

        let expires_at = parse_ts(&expires_at);
        if expires_at <= now() {
            return Ok(PairingCodeStatus::Expired { id, org_id });
        }

        let attempts = u32::try_from(attempts).unwrap_or(u32::MAX);
        if attempts >= max_attempts {
            return Ok(PairingCodeStatus::Locked { id, org_id });
        }

        Ok(PairingCodeStatus::Valid(PairingCodeInfo {
            id,
            org_id,
            created_by,
            machine_name_hint,
            expires_at,
            attempts_left: max_attempts - attempts,
        }))
    }

    /// Count a failed signature attempt and append a `machine.pair_denied`
    /// audit entry for the owning organization.
    ///
    /// Both writes happen in one immediate transaction. An unknown code id is a
    /// no-op: it still returns `Ok(())` but writes no audit row.
    pub async fn record_failure(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT org_id, created_by FROM pairing_codes WHERE id = ?1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((org_id, created_by)) = row else {
            tx.rollback().await?;
            return Ok(());
        };

        sqlx::query("UPDATE pairing_codes SET attempts = attempts + 1 WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            r#"
            INSERT INTO audit_logs (actor, org_id, action, subject, details_json, created_at)
            VALUES (?1, ?2, 'machine.pair_denied', ?3, ?4, ?5)
            "#,
        )
        .bind(&created_by)
        .bind(&org_id)
        .bind(id)
        .bind(
            serde_json::json!({
                "code_id": id,
                "outcome": "denied",
                "reason": "signature",
            })
            .to_string(),
        )
        .bind(format_ts(now()))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }
}

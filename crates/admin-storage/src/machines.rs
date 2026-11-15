//! Machines storage operations.

use fleet_core_common::{now, Timestamp};
use fleet_core_protocol::machine::{Machine, MachineStatus, RevokedMachine};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

use crate::rows::MachineRow;

/// Handle for machines queries.
#[derive(Clone)]
pub struct Machines {
    pool: SqlitePool,
}

impl Machines {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Machines {
    /// Insert a machine or replace its mutable fields.
    pub async fn upsert(&self, m: &Machine) -> Result<()> {
        let providers_json = serde_json::to_string(&m.providers)?;
        let workspaces_json = serde_json::to_string(&m.workspaces)?;

        sqlx::query(
            r#"
            INSERT INTO machines (
                id, name, hostname, os, arch, daemon_version, protocol_version,
                status, public_key, connected_at, last_seen_at, providers_json,
                workspaces_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                hostname = excluded.hostname,
                os = excluded.os,
                arch = excluded.arch,
                daemon_version = excluded.daemon_version,
                protocol_version = excluded.protocol_version,
                status = excluded.status,
                public_key = excluded.public_key,
                connected_at = excluded.connected_at,
                last_seen_at = excluded.last_seen_at,
                providers_json = excluded.providers_json,
                workspaces_json = excluded.workspaces_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&m.id)
        .bind(&m.name)
        .bind(&m.hostname)
        .bind(&m.os)
        .bind(&m.arch)
        .bind(&m.daemon_version)
        .bind(i64::from(m.protocol_version))
        .bind(machine_status_text(m.status))
        .bind(&m.public_key)
        .bind(format_opt_ts(m.connected_at))
        .bind(format_opt_ts(m.last_seen_at))
        .bind(providers_json)
        .bind(workspaces_json)
        .bind(format_ts(m.created_at))
        .bind(format_ts(m.updated_at))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch one machine by id.
    pub async fn get(&self, id: &str) -> Result<Option<Machine>> {
        let row = sqlx::query_as::<_, MachineRow>(
            r#"
            SELECT id, name, hostname, os, arch, daemon_version, protocol_version,
                   status, public_key, connected_at, last_seen_at, providers_json,
                   workspaces_json, created_at, updated_at
            FROM machines
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(MachineRow::into_machine))
    }

    /// List every known machine, ordered by name.
    pub async fn list(&self) -> Result<Vec<Machine>> {
        let rows = sqlx::query_as::<_, MachineRow>(
            r#"
            SELECT id, name, hostname, os, arch, daemon_version, protocol_version,
                   status, public_key, connected_at, last_seen_at, providers_json,
                   workspaces_json, created_at, updated_at
            FROM machines
            ORDER BY name ASC, id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(MachineRow::into_machine).collect())
    }

    /// Update liveness fields for a machine.
    ///
    /// `None` timestamp arguments keep the currently stored value.
    pub async fn update_status(
        &self,
        id: &str,
        status: MachineStatus,
        connected_at: Option<Timestamp>,
        last_seen_at: Option<Timestamp>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE machines
            SET status = ?2,
                connected_at = COALESCE(?3, connected_at),
                last_seen_at = COALESCE(?4, last_seen_at),
                updated_at = ?5
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .bind(machine_status_text(status))
        .bind(format_opt_ts(connected_at))
        .bind(format_opt_ts(last_seen_at))
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Revoke a machine id so a daemon cannot re-pair by reconnecting.
    ///
    /// The owning organization is copied from `machine_orgs` before the machine
    /// rows are deleted, so an organization administrator can later list and
    /// manage only their own revoked machines.
    pub async fn revoke(&self, machine_id: &str, reason: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO machine_revocations (machine_id, revoked_at, reason, org_id, public_key)
            VALUES (
                ?1,
                ?2,
                ?3,
                (SELECT org_id FROM machine_orgs WHERE machine_id = ?1),
                (SELECT public_key FROM machines WHERE id = ?1)
            )
            ON CONFLICT(machine_id) DO UPDATE SET
                revoked_at = excluded.revoked_at,
                reason = excluded.reason,
                org_id = COALESCE(excluded.org_id, machine_revocations.org_id),
                public_key = COALESCE(excluded.public_key, machine_revocations.public_key)
            "#,
        )
        .bind(machine_id)
        .bind(format_ts(now()))
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove a revocation. Returns false when the machine was not revoked.
    pub async fn unrevoke(&self, machine_id: &str) -> Result<bool> {
        self.unrevoke_scoped(machine_id, None).await
    }

    /// Remove a revocation only when it belongs to `org_id` (or any revocation
    /// for the legacy super-admin, who passes `None`).
    pub async fn unrevoke_scoped(&self, machine_id: &str, org_id: Option<&str>) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM machine_revocations WHERE machine_id = ?1 AND (?2 IS NULL OR org_id = ?2)",
        )
        .bind(machine_id)
        .bind(org_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Organization recorded when the machine was revoked.
    pub async fn revocation_org(&self, machine_id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT org_id FROM machine_revocations WHERE machine_id = ?1")
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|row| row.0))
    }

    /// Pinned public key recorded when the machine was revoked.
    pub async fn revocation_public_key(&self, machine_id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT public_key FROM machine_revocations WHERE machine_id = ?1")
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|row| row.0))
    }

    /// Whether a machine id is revoked.
    pub async fn is_revoked(&self, machine_id: &str) -> Result<bool> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT machine_id FROM machine_revocations WHERE machine_id = ?1")
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    /// List revoked machines, newest first.
    pub async fn list_revoked(&self) -> Result<Vec<RevokedMachine>> {
        self.list_revoked_scoped(None).await
    }

    /// List revoked machines visible to one organization (`None` means every
    /// organization and is reserved for the legacy super-admin).
    pub async fn list_revoked_scoped(&self, org_id: Option<&str>) -> Result<Vec<RevokedMachine>> {
        let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT machine_id, revoked_at, reason
            FROM machine_revocations
            WHERE (?1 IS NULL OR org_id = ?1)
            ORDER BY revoked_at DESC, machine_id ASC
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(machine_id, revoked_at, reason)| RevokedMachine {
                machine_id,
                revoked_at: parse_ts(&revoked_at),
                reason,
            })
            .collect())
    }

    /// Delete a machine row.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (sql, bind) in [
            (
                "DELETE FROM permission_requests WHERE session_id IN (SELECT id FROM agent_sessions WHERE machine_id = ?1)",
                id,
            ),
            ("DELETE FROM agent_sessions WHERE machine_id = ?1", id),
            ("DELETE FROM workspaces WHERE machine_id = ?1", id),
            ("DELETE FROM machine_orgs WHERE machine_id = ?1", id),
            ("DELETE FROM machines WHERE id = ?1", id),
        ] {
            sqlx::query(sql).bind(bind).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

//! Machines storage operations.

use fleet_core_auth::hash_token;
use fleet_core_common::{error_codes, now, ApiError, Timestamp};
use fleet_core_protocol::agent::ProviderInfo;
use fleet_core_protocol::machine::{Machine, MachineStatus, RevokedMachine};
use fleet_core_protocol::pairing::{MachineAuthState, PairingCompletion, PairingOutcome};
use fleet_core_protocol::workspace::WorkspaceSummary;
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
                -- The pinned key is owned by the pairing/rotation path. Routine
                -- registration (which writes a cached snapshot) may only fill
                -- in a missing key, never replace a committed one.
                public_key = COALESCE(machines.public_key, excluded.public_key),
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

    /// Replace the provider snapshot without touching pairing-owned columns.
    pub async fn update_providers(&self, id: &str, providers: &[ProviderInfo]) -> Result<()> {
        let json = serde_json::to_string(providers)?;
        sqlx::query("UPDATE machines SET providers_json = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(json)
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Replace the workspace snapshot without touching pairing-owned columns.
    pub async fn update_workspaces(&self, id: &str, workspaces: &[WorkspaceSummary]) -> Result<()> {
        let json = serde_json::to_string(workspaces)?;
        sqlx::query("UPDATE machines SET workspaces_json = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(json)
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Destroy the per-machine credential, including any unconfirmed
    /// rotation. Revocation and unrevocation both call this: a re-pair is
    /// required to obtain a new token.
    pub async fn clear_token(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE machines
             SET token_hash = NULL, token_created_at = NULL, pending_token_hash = NULL
             WHERE id = ?1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether the machine has a live token that can be rotated.
    pub async fn has_token(&self, id: &str) -> Result<bool> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT token_hash FROM machines WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(hash,)| hash).is_some())
    }

    /// Record the hash of a token that has been offered to the daemon but not
    /// confirmed yet. While it is set, both it and `token_hash` authenticate;
    /// [`Self::promote_pending_token`] invalidates the old token.
    pub async fn set_pending_token(&self, id: &str, token_hash: &str) -> Result<bool> {
        // Only a machine with a live token may start a rotation. The guard
        // also loses a race with unrevocation, which clears the token: a
        // rotation must never re-issue a credential to a machine that has
        // been told to pair again.
        let result = sqlx::query(
            "UPDATE machines SET pending_token_hash = ?2, updated_at = ?3
             WHERE id = ?1 AND token_hash IS NOT NULL",
        )
        .bind(id)
        .bind(token_hash)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Hash of the unconfirmed rotation, when one is stored.
    pub async fn pending_token_hash(&self, id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT pending_token_hash FROM machines WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(hash,)| hash))
    }

    /// Resolve a token hash offered by a daemon to the machine that requested
    /// the rotation. Used only for authentication, never for authorization.
    pub async fn by_pending_token_hash(&self, token_hash: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM machines WHERE pending_token_hash = ?1")
                .bind(token_hash)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }

    /// Promote a confirmed rotation to the live credential.
    ///
    /// The compare-and-set on the expected hash makes this safe when the
    /// daemon reconnects with the pending token at the same time as the admin
    /// records its acknowledgement. Returns false when there is nothing to
    /// promote (already promoted or canceled).
    pub async fn promote_pending_token(&self, id: &str, token_hash: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE machines
             SET token_hash = pending_token_hash,
                 token_created_at = ?3,
                 pending_token_hash = NULL,
                 updated_at = ?3
             WHERE id = ?1 AND pending_token_hash = ?2",
        )
        .bind(id)
        .bind(token_hash)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Abandon an unconfirmed rotation. The current token stays valid.
    pub async fn clear_pending_token(&self, id: &str) -> Result<()> {
        sqlx::query("UPDATE machines SET pending_token_hash = NULL WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Pin a public key for an existing machine without rewriting other columns.
    pub async fn set_public_key(&self, id: &str, public_key: &str) -> Result<()> {
        sqlx::query("UPDATE machines SET public_key = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(public_key)
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
        // One transaction: the revocation and the destruction of the credential
        // commit together, so a crash before the caller deletes the machine row
        // cannot leave a token that `unrevoke` would silently revive.
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO machine_revocations (
                machine_id, revoked_at, reason, org_id, public_key, machine_name
            )
            VALUES (
                ?1,
                ?2,
                ?3,
                (SELECT org_id FROM machine_orgs WHERE machine_id = ?1),
                (SELECT public_key FROM machines WHERE id = ?1),
                (SELECT name FROM machines WHERE id = ?1)
            )
            ON CONFLICT(machine_id) DO UPDATE SET
                revoked_at = excluded.revoked_at,
                reason = excluded.reason,
                org_id = COALESCE(excluded.org_id, machine_revocations.org_id),
                public_key = COALESCE(excluded.public_key, machine_revocations.public_key),
                machine_name = COALESCE(excluded.machine_name, machine_revocations.machine_name)
            "#,
        )
        .bind(machine_id)
        .bind(format_ts(now()))
        .bind(reason)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE machines
             SET token_hash = NULL, token_created_at = NULL, pending_token_hash = NULL
             WHERE id = ?1",
        )
        .bind(machine_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Remove a revocation. Returns false when the machine was not revoked.
    pub async fn unrevoke(&self, machine_id: &str) -> Result<bool> {
        self.unrevoke_scoped(machine_id, None).await
    }

    /// Remove a revocation only when it belongs to `org_id` (or any revocation
    /// for the legacy super-admin, who passes `None`).
    ///
    /// The revocation delete and the token clear run in one transaction: a
    /// crash can never leave the token live with the revocation already gone
    /// (which would make an older, pre-fix revocation silently reconnect).
    pub async fn unrevoke_scoped(&self, machine_id: &str, org_id: Option<&str>) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "DELETE FROM machine_revocations WHERE machine_id = ?1 AND (?2 IS NULL OR org_id = ?2)",
        )
        .bind(machine_id)
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE machines
             SET token_hash = NULL, token_created_at = NULL, pending_token_hash = NULL
             WHERE id = ?1",
        )
        .bind(machine_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
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
        let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT machine_id, revoked_at, reason, machine_name
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
            .map(
                |(machine_id, revoked_at, reason, machine_name)| RevokedMachine {
                    machine_id,
                    revoked_at: parse_ts(&revoked_at),
                    reason,
                    machine_name,
                },
            )
            .collect())
    }

    /// Delete a machine row.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (sql, bind) in [
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

    /// Pinned key for a machine id, or `None` when the machine is unknown.
    pub async fn machine_state(&self, machine_id: &str) -> Result<MachineAuthState> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT public_key FROM machines WHERE id = ?1")
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(MachineAuthState {
            pinned_key: row.and_then(|(key,)| key),
        })
    }

    /// Resolve a per-machine token hash to its machine id.
    pub async fn by_token_hash(&self, token_hash: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM machines WHERE token_hash = ?1")
                .bind(token_hash)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }

    /// Atomically consume a pairing code, pin the key and issue a token hash.
    pub async fn complete_pairing(
        &self,
        request: PairingCompletion,
        max_attempts: u32,
    ) -> std::result::Result<PairingOutcome, ApiError> {
        // BEGIN IMMEDIATE takes the write lock up front. A concurrent pairing
        // therefore waits here instead of failing on a read-to-write upgrade
        // (`SQLITE_BUSY_SNAPSHOT`), then observes `used_at` and reports
        // `pairing_code_invalid` rather than an internal error.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(pairing_begin_error)?;

        // Intentional deviation from spec §9.2 step 1: consume is inlined here
        // rather than extracted as `PairingCodes::consume`, and it runs only
        // after the revocation/key/org checks. That ordering is safer: a
        // rejected pairing must not burn the code.
        let code: Option<(String, String, Option<String>, String, Option<String>, i64)> =
            sqlx::query_as(
                r#"
                SELECT org_id, created_by, machine_name_hint, expires_at, used_at, attempts
                FROM pairing_codes
                WHERE id = ?1
                "#,
            )
            .bind(&request.code_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_error)?;

        let Some((org_id, created_by, machine_name_hint, expires_at, used_at, attempts)) = code
        else {
            return Err(pairing_invalid());
        };
        if used_at.is_some()
            || parse_ts(&expires_at) <= now()
            || u32::try_from(attempts).unwrap_or(u32::MAX) >= max_attempts
        {
            return Err(pairing_invalid());
        }

        let revoked: Option<(String,)> =
            sqlx::query_as("SELECT machine_id FROM machine_revocations WHERE machine_id = ?1")
                .bind(&request.machine_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_error)?;
        if revoked.is_some() {
            return Err(ApiError::new(
                error_codes::FORBIDDEN,
                format!("machine {} is revoked", request.machine_id),
            ));
        }

        let existing: Option<(Option<String>,)> =
            sqlx::query_as("SELECT public_key FROM machines WHERE id = ?1")
                .bind(&request.machine_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_error)?;

        if let Some((Some(pinned),)) = existing.as_ref() {
            if pinned != &request.public_key && !request.rotation_verified {
                return Err(ApiError::new(
                    error_codes::MACHINE_KEY_MISMATCH,
                    "machine is already paired with a different key",
                ));
            }
        }

        // A null owner means the previous organization was deleted. The
        // machine is still registered and may be re-paired into a new one.
        let existing_org: Option<(Option<String>,)> =
            sqlx::query_as("SELECT org_id FROM machine_orgs WHERE machine_id = ?1")
                .bind(&request.machine_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_error)?;
        if let Some((Some(existing_org),)) = existing_org.as_ref() {
            if existing_org != &org_id {
                return Err(ApiError::new(
                    error_codes::MACHINE_ORG_MISMATCH,
                    "machine belongs to another organization",
                ));
            }
        }

        let consumed_at = now();
        let consumed = sqlx::query(
            r#"
            UPDATE pairing_codes
            SET used_at = ?1, used_by_machine_id = ?2
            WHERE id = ?3 AND used_at IS NULL AND expires_at > ?4
            "#,
        )
        .bind(format_ts(consumed_at))
        .bind(&request.machine_id)
        .bind(&request.code_id)
        .bind(format_ts(consumed_at))
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
        if consumed.rows_affected() != 1 {
            return Err(pairing_invalid());
        }

        let timestamp = now();
        let name = machine_name_hint.unwrap_or_else(|| request.machine_name.clone());

        if existing.is_some() {
            sqlx::query(
                r#"
                UPDATE machines
                SET name = ?2, hostname = ?3, os = ?4, arch = ?5,
                    daemon_version = ?6, protocol_version = ?7, public_key = ?8,
                    token_hash = ?9, token_created_at = ?10, pending_token_hash = NULL,
                    paired_at = ?11, paired_by = ?12, updated_at = ?13
                WHERE id = ?1
                "#,
            )
            .bind(&request.machine_id)
            .bind(&name)
            .bind(&request.hostname)
            .bind(&request.os)
            .bind(&request.arch)
            .bind(&request.daemon_version)
            .bind(i64::from(request.protocol_version))
            .bind(&request.public_key)
            .bind(&request.token_hash)
            .bind(format_ts(timestamp))
            .bind(format_ts(timestamp))
            .bind(&created_by)
            .bind(format_ts(timestamp))
            .execute(&mut *tx)
            .await
            .map_err(internal_error)?;
        } else {
            sqlx::query(
                r#"
                INSERT INTO machines (
                    id, name, hostname, os, arch, daemon_version, protocol_version,
                    status, public_key, providers_json, workspaces_json,
                    created_at, updated_at, token_hash, token_created_at,
                    paired_at, paired_by
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'offline', ?8, '[]', '[]',
                    ?9, ?10, ?11, ?12, ?13, ?14
                )
                "#,
            )
            .bind(&request.machine_id)
            .bind(&name)
            .bind(&request.hostname)
            .bind(&request.os)
            .bind(&request.arch)
            .bind(&request.daemon_version)
            .bind(i64::from(request.protocol_version))
            .bind(&request.public_key)
            .bind(format_ts(timestamp))
            .bind(format_ts(timestamp))
            .bind(&request.token_hash)
            .bind(format_ts(timestamp))
            .bind(format_ts(timestamp))
            .bind(&created_by)
            .execute(&mut *tx)
            .await
            .map_err(internal_error)?;
        }

        sqlx::query(
            r#"
            INSERT INTO machine_orgs (machine_id, org_id, created_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(machine_id) DO UPDATE SET org_id = excluded.org_id
            "#,
        )
        .bind(&request.machine_id)
        .bind(&org_id)
        .bind(format_ts(timestamp))
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;

        sqlx::query(
            r#"
            INSERT INTO audit_logs (actor, org_id, action, subject, details_json, created_at)
            VALUES (?1, ?2, 'machine.paired', ?3, ?4, ?5)
            "#,
        )
        .bind(&created_by)
        .bind(&org_id)
        .bind(&request.machine_id)
        .bind(
            serde_json::json!({
                "code_id": request.code_id,
                "public_key_fingerprint": hash_token(&request.public_key),
                "outcome": "ok",
            })
            .to_string(),
        )
        .bind(format_ts(timestamp))
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;

        tx.commit().await.map_err(internal_error)?;
        Ok(PairingOutcome {
            machine_id: request.machine_id,
            org_id,
            paired_at: timestamp,
        })
    }
}

fn internal_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error.to_string())
}

/// Map a failure to acquire the pairing write lock to a retryable pairing
/// error rather than an internal error. `busy_timeout` normally hides this,
/// but an I/O stall can still surface `SQLITE_BUSY`.
fn pairing_begin_error(error: sqlx::Error) -> ApiError {
    if is_busy_message(&error.to_string()) {
        pairing_invalid()
    } else {
        internal_error(error)
    }
}

fn is_busy_message(message: &str) -> bool {
    message.contains("SQLITE_BUSY")
        || message.contains("database is locked")
        || message.contains("database table is locked")
}

fn pairing_invalid() -> ApiError {
    ApiError::new(
        error_codes::PAIRING_CODE_INVALID,
        "pairing code is not valid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_errors_map_to_pairing_invalid() {
        assert!(is_busy_message("SQLITE_BUSY: database is locked"));
        assert!(is_busy_message("database table is locked"));
        assert!(!is_busy_message("disk I/O error"));
    }
}

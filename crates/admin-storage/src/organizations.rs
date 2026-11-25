//! Organizations storage operations.

use fleet_core_common::now;
use fleet_core_protocol::authz::{Organization, OrganizationKind};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

use crate::rows::OrganizationRow;

/// Handle for organizations queries.
#[derive(Clone)]
pub struct Organizations {
    pool: SqlitePool,
}

impl Organizations {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, name, kind, owner_user_id, deleted_at, created_at, updated_at";

/// One organization removed by the scheduled retention sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgedOrganization {
    /// Organization id.
    pub id: String,
    /// Display name recorded before the row disappeared.
    pub name: String,
    /// When the organization was soft-deleted.
    pub deleted_at: fleet_core_common::Timestamp,
}

/// Outcome of a non-cascading organization soft delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationDeletion {
    /// Machines that lost their organization and are now unowned.
    pub detached_machines: Vec<String>,
    /// Memberships revoked with the organization.
    pub deleted_memberships: u64,
    /// Unused pairing codes removed with the organization.
    pub deleted_pairing_codes: u64,
    /// Pending invitations removed with the organization.
    pub deleted_invitations: u64,
}

impl Organizations {
    /// Create an organization with a generated id.
    ///
    /// A personal space gets `kind = Personal` and the owning user recorded;
    /// team organizations record the creating user as both owner and owner
    /// membership is inserted by the caller.
    pub async fn create(
        &self,
        kind: OrganizationKind,
        name: &str,
        owner_user_id: Option<&str>,
    ) -> Result<Organization> {
        let timestamp = now();
        let organization = Organization {
            id: fleet_core_common::new_id("org"),
            name: name.to_string(),
            kind,
            owner_user_id: owner_user_id.map(str::to_owned),
            deleted_at: None,
            created_at: timestamp,
            updated_at: timestamp,
        };
        sqlx::query(
            r#"
            INSERT INTO organizations
                (id, name, kind, owner_user_id, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(&organization.id)
        .bind(&organization.name)
        .bind(kind.as_str())
        .bind(&organization.owner_user_id)
        .bind(format_ts(timestamp))
        .bind(format_ts(timestamp))
        .execute(&self.pool)
        .await?;
        Ok(organization)
    }

    /// Insert the default organization when it does not exist yet.
    pub async fn ensure_default(&self) -> Result<Organization> {
        let id = fleet_core_protocol::authz::DEFAULT_ORG_ID;
        if let Some(organization) = self.get(id).await? {
            return Ok(organization);
        }
        let timestamp = now();
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO organizations
                (id, name, kind, created_at, updated_at)
            VALUES (?1, ?2, 'team', ?3, ?4)
            "#,
        )
        .bind(id)
        .bind("Default")
        .bind(format_ts(timestamp))
        .bind(format_ts(timestamp))
        .execute(&self.pool)
        .await?;
        self.get(id).await?.ok_or_else(|| {
            crate::StorageError::InvalidData(
                "failed to read the default organization after inserting it".to_string(),
            )
        })
    }

    /// Fetch one organization, including soft-deleted ones.
    pub async fn get(&self, id: &str) -> Result<Option<Organization>> {
        let row: Option<OrganizationRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM organizations WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| Organization::try_from(row).ok()))
    }

    /// List active (not soft-deleted) organizations oldest first.
    pub async fn list(&self) -> Result<Vec<Organization>> {
        let rows: Vec<OrganizationRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM organizations WHERE deleted_at IS NULL ORDER BY created_at"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| Organization::try_from(row).ok())
            .collect())
    }

    /// The personal organization of one user, if created.
    pub async fn personal_for_user(&self, user_id: &str) -> Result<Option<Organization>> {
        let row: Option<OrganizationRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM organizations \
             WHERE kind = 'personal' AND owner_user_id = ?1 AND deleted_at IS NULL"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| Organization::try_from(row).ok()))
    }

    /// Rename an organization.
    pub async fn rename(&self, id: &str, name: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE organizations SET name = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(name)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Record a new owner. Owner membership is managed separately.
    pub async fn set_owner(&self, id: &str, user_id: Option<&str>) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE organizations SET owner_user_id = ?2, updated_at = ?3 WHERE id = ?1",
        )
        .bind(id)
        .bind(user_id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Soft-delete an organization without cascading into machines or history.
    ///
    /// In one transaction this detaches the organization's machines, leaving
    /// their rows registered with a null owner, revokes every membership,
    /// deletes unused pairing codes, and removes pending invitations. Machines,
    /// workspaces, agent sessions, used pairing codes and audit rows are kept.
    /// Returns `None` when the organization was already deleted.
    pub async fn soft_delete_detaching(&self, id: &str) -> Result<Option<OrganizationDeletion>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let timestamp = format_ts(now());
        let result = sqlx::query(
            "UPDATE organizations SET deleted_at = ?2, updated_at = ?2 \
             WHERE id = ?1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(&timestamp)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(None);
        }

        let machines: Vec<(String,)> =
            sqlx::query_as("SELECT machine_id FROM machine_orgs WHERE org_id = ?1")
                .bind(id)
                .fetch_all(&mut *tx)
                .await?;
        sqlx::query("UPDATE machine_orgs SET org_id = NULL WHERE org_id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // Access ends with the soft delete, not with the later purge. This also
        // keeps every membership-bearing endpoint from resolving a deleted
        // organization as the caller's active one.
        let memberships = sqlx::query("DELETE FROM memberships WHERE org_id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let pairing_codes =
            sqlx::query("DELETE FROM pairing_codes WHERE org_id = ?1 AND used_at IS NULL")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        let invitations =
            sqlx::query("DELETE FROM org_invitations WHERE org_id = ?1 AND accepted_at IS NULL")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        tx.commit().await?;

        Ok(Some(OrganizationDeletion {
            detached_machines: machines.into_iter().map(|row| row.0).collect(),
            deleted_memberships: memberships.rows_affected(),
            deleted_pairing_codes: pairing_codes.rows_affected(),
            deleted_invitations: invitations.rows_affected(),
        }))
    }

    /// Permanently remove every organization soft-deleted at or before
    /// `cutoff`, returning the removed rows.
    ///
    /// Memberships, invitations and pairing codes cascade with the row.
    /// Machines, workspaces, agent sessions and audit rows are kept;
    /// `machine_orgs.org_id` becomes null. The deletion and its `org.purge`
    /// audit row commit together, so the log cannot lose a purge that
    /// happened. The whole sweep is one immediate transaction.
    pub async fn purge_expired(
        &self,
        cutoff: fleet_core_common::Timestamp,
        actor: &str,
    ) -> Result<Vec<PurgedOrganization>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, name, deleted_at FROM organizations WHERE deleted_at IS NOT NULL",
        )
        .fetch_all(&mut *tx)
        .await?;

        let mut purged = Vec::new();
        for (id, name, deleted_at) in rows {
            let deleted_at = parse_ts(&deleted_at);
            if deleted_at > cutoff {
                continue;
            }
            let result =
                sqlx::query("DELETE FROM organizations WHERE id = ?1 AND deleted_at IS NOT NULL")
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
            if result.rows_affected() == 0 {
                continue;
            }
            let details = serde_json::json!({
                "name": &name,
                "deleted_at": deleted_at.to_rfc3339(),
            });
            sqlx::query(
                "INSERT INTO audit_logs \
                 (actor, org_id, action, subject, details_json, created_at) \
                 VALUES (?1, NULL, 'org.purge', ?2, ?3, ?4)",
            )
            .bind(actor)
            .bind(&id)
            .bind(details.to_string())
            .bind(format_ts(now()))
            .execute(&mut *tx)
            .await?;
            purged.push(PurgedOrganization {
                id,
                name,
                deleted_at,
            });
        }
        tx.commit().await?;
        Ok(purged)
    }
}

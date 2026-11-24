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

    /// Soft-delete an organization. The caller must also disconnect machines or
    /// move them; this only marks the row.
    pub async fn soft_delete(&self, id: &str) -> Result<bool> {
        let timestamp = now();
        let result = sqlx::query(
            "UPDATE organizations SET deleted_at = ?2, updated_at = ?2 \
             WHERE id = ?1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(format_ts(timestamp))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Permanently remove a soft-deleted organization row. Machines are left
    /// unowned by the `machine_orgs` foreign key.
    pub async fn purge(&self, id: &str) -> Result<bool> {
        let result =
            sqlx::query("DELETE FROM organizations WHERE id = ?1 AND deleted_at IS NOT NULL")
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }
}

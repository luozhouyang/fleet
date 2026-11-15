//! Organizations storage operations.

use fleet_core_common::now;
use fleet_core_protocol::authz::Organization;
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

impl Organizations {
    /// Create an organization with a generated id.
    pub async fn create(&self, name: &str) -> Result<Organization> {
        let organization = Organization {
            id: fleet_core_common::new_id("org"),
            name: name.to_string(),
            created_at: now(),
        };
        sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES (?1, ?2, ?3)")
            .bind(&organization.id)
            .bind(&organization.name)
            .bind(format_ts(organization.created_at))
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
        let organization = Organization {
            id: id.to_string(),
            name: "Default".to_string(),
            created_at: now(),
        };
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO organizations (id, name, created_at)
            VALUES (?1, ?2, ?3)
            "#,
        )
        .bind(&organization.id)
        .bind(&organization.name)
        .bind(format_ts(organization.created_at))
        .execute(&self.pool)
        .await?;
        self.get(id).await?.ok_or_else(|| {
            crate::StorageError::InvalidData(
                "failed to read the default organization after inserting it".to_string(),
            )
        })
    }

    /// Fetch one organization.
    pub async fn get(&self, id: &str) -> Result<Option<Organization>> {
        let row: Option<OrganizationRow> =
            sqlx::query_as("SELECT id, name, created_at FROM organizations WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(Organization::from))
    }

    /// List organizations oldest first.
    pub async fn list(&self) -> Result<Vec<Organization>> {
        let rows: Vec<OrganizationRow> =
            sqlx::query_as("SELECT id, name, created_at FROM organizations ORDER BY created_at")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(Organization::from).collect())
    }
}

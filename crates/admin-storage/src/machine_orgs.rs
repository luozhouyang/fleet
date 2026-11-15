//! MachineOrgs storage operations.

use fleet_core_common::now;
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

/// Handle for machine_orgs queries.
#[derive(Clone)]
pub struct MachineOrgs {
    pool: SqlitePool,
}

impl MachineOrgs {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl MachineOrgs {
    /// Assign (or reassign) a machine to an organization.
    pub async fn assign(&self, machine_id: &str, org_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO machine_orgs (machine_id, org_id, created_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(machine_id) DO UPDATE SET org_id = excluded.org_id
            "#,
        )
        .bind(machine_id)
        .bind(org_id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Assign a machine to an organization only when it has no owner yet.
    pub async fn ensure(&self, machine_id: &str, org_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO machine_orgs (machine_id, org_id, created_at)
            VALUES (?1, ?2, ?3)
            "#,
        )
        .bind(machine_id)
        .bind(org_id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Organization that owns a machine, if any.
    pub async fn get(&self, machine_id: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT org_id FROM machine_orgs WHERE machine_id = ?1")
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|row| row.0))
    }

    /// Machine ids owned by one organization.
    pub async fn ids_for_org(&self, org_id: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT machine_id FROM machine_orgs WHERE org_id = ?1")
                .bind(org_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
}

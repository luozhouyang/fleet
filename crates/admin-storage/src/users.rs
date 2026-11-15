//! Users storage operations.

use fleet_core_common::now;
use fleet_core_protocol::authz::{User, UserRole};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

use crate::rows::UserRow;

/// Handle for users queries.
#[derive(Clone)]
pub struct Users {
    pool: SqlitePool,
}

impl Users {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Users {
    /// Insert a user. `token_hash` must already be a one-way hash.
    pub async fn create(
        &self,
        org_id: &str,
        name: &str,
        role: UserRole,
        token_hash: &str,
    ) -> Result<User> {
        let timestamp = now();
        let user = User {
            id: fleet_core_common::new_id("user"),
            org_id: org_id.to_string(),
            name: name.to_string(),
            role,
            created_at: timestamp,
            updated_at: timestamp,
        };
        sqlx::query(
            r#"
            INSERT INTO users (id, org_id, name, role, token_hash, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&user.id)
        .bind(&user.org_id)
        .bind(&user.name)
        .bind(user.role.as_str())
        .bind(token_hash)
        .bind(format_ts(timestamp))
        .bind(format_ts(timestamp))
        .execute(&self.pool)
        .await?;
        Ok(user)
    }

    /// List users, optionally restricted to one organization.
    pub async fn list(&self, org_id: Option<&str>) -> Result<Vec<User>> {
        let rows: Vec<UserRow> = match org_id {
            Some(org_id) => {
                sqlx::query_as(
                    "SELECT id, org_id, name, role, created_at, updated_at FROM users                      WHERE org_id = ?1 ORDER BY created_at",
                )
                .bind(org_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT id, org_id, name, role, created_at, updated_at FROM users                      ORDER BY created_at",
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows
            .into_iter()
            .filter_map(|row| User::try_from(row).ok())
            .collect())
    }

    /// Fetch one user.
    pub async fn get(&self, id: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT id, org_id, name, role, created_at, updated_at FROM users WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| User::try_from(row).ok()))
    }

    /// Resolve a user by the hash of their bearer token.
    pub async fn by_token_hash(&self, token_hash: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT id, org_id, name, role, created_at, updated_at FROM users              WHERE token_hash = ?1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| User::try_from(row).ok()))
    }

    /// Delete a user. Returns false when it did not exist.
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Replace a user's token hash. Returns false when the user does not exist.
    pub async fn update_token_hash(&self, id: &str, token_hash: &str) -> Result<bool> {
        let result = sqlx::query("UPDATE users SET token_hash = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(token_hash)
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

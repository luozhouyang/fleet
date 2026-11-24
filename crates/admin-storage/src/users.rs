//! Users storage operations.
//!
//! A user is global: email, display name, avatar and an optional CLI bearer
//! token. Organization membership lives in [`crate::memberships`].

use fleet_core_common::{now, Timestamp};
use fleet_core_protocol::authz::User;
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

const COLUMNS: &str = "id, email, name, avatar_url, created_at, updated_at, \
                       token_issued_at, token_expires_at, disabled_at";

impl Users {
    /// Insert a user. `token_hash` must already be a one-way hash.
    pub async fn create(
        &self,
        email: Option<&str>,
        name: &str,
        avatar_url: Option<&str>,
        token_hash: Option<&str>,
        token_issued_at: Option<Timestamp>,
        token_expires_at: Option<Timestamp>,
    ) -> Result<User> {
        let timestamp = now();
        let user = User {
            id: fleet_core_common::new_id("user"),
            email: email.map(str::to_owned),
            name: name.to_string(),
            avatar_url: avatar_url.map(str::to_owned),
            created_at: timestamp,
            updated_at: timestamp,
            token_issued_at,
            token_expires_at,
            disabled_at: None,
        };
        sqlx::query(
            r#"
            INSERT INTO users (
                id, email, name, avatar_url, token_hash, created_at, updated_at,
                token_issued_at, token_expires_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8)
            "#,
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.avatar_url)
        .bind(token_hash)
        .bind(format_ts(timestamp))
        .bind(format_opt_ts(token_issued_at))
        .bind(format_opt_ts(token_expires_at))
        .execute(&self.pool)
        .await?;
        Ok(user)
    }

    /// Insert a user that was created by an OAuth sign-in.
    pub async fn create_from_identity(
        &self,
        email: &str,
        name: &str,
        avatar_url: Option<&str>,
    ) -> Result<User> {
        self.create(Some(email), name, avatar_url, None, None, None)
            .await
    }

    /// List every active user.
    pub async fn list(&self) -> Result<Vec<User>> {
        let rows: Vec<UserRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM users WHERE disabled_at IS NULL ORDER BY created_at"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| User::try_from(row).ok())
            .collect())
    }

    /// Fetch one user, including disabled accounts.
    pub async fn get(&self, id: &str) -> Result<Option<User>> {
        let row: Option<UserRow> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM users WHERE id = ?1"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|row| User::try_from(row).ok()))
    }

    /// Fetch one user by normalized email.
    pub async fn by_email(&self, email: &str) -> Result<Option<User>> {
        let row: Option<UserRow> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM users WHERE email = ?1"))
                .bind(normalize_email(email))
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|row| User::try_from(row).ok()))
    }

    /// Resolve a user by the hash of their bearer token.
    pub async fn by_token_hash(&self, token_hash: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM users WHERE token_hash = ?1"
        ))
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| User::try_from(row).ok()))
    }

    /// Whether any user has ever been created. Used by the OAuth bootstrap.
    pub async fn any_exists(&self) -> Result<bool> {
        let row: Option<(String,)> = sqlx::query_as("SELECT id FROM users LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    /// Update the profile fields an identity provider owns.
    pub async fn update_profile(
        &self,
        id: &str,
        name: &str,
        avatar_url: Option<&str>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE users SET name = ?2, avatar_url = ?3, updated_at = ?4 WHERE id = ?1",
        )
        .bind(id)
        .bind(name)
        .bind(avatar_url)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Link or refresh the verified email of a user.
    pub async fn set_email(&self, id: &str, email: &str) -> Result<bool> {
        let result = sqlx::query("UPDATE users SET email = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(normalize_email(email))
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Permanently delete a user. Memberships and sessions cascade; machines,
    /// workspaces and audit rows are untouched.
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Disable a user without deleting the account.
    pub async fn disable(&self, id: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE users SET disabled_at = ?2, updated_at = ?2 \
             WHERE id = ?1 AND disabled_at IS NULL",
        )
        .bind(id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Replace a user's token and its lifetime. Returns false when the user
    /// does not exist.
    pub async fn update_token(
        &self,
        id: &str,
        token_hash: &str,
        token_issued_at: Timestamp,
        token_expires_at: Option<Timestamp>,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE users
            SET token_hash = ?2, token_issued_at = ?3, token_expires_at = ?4,
                updated_at = ?5
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .bind(token_hash)
        .bind(format_ts(token_issued_at))
        .bind(format_opt_ts(token_expires_at))
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Normalize an email for storage and lookup: trim and lowercase.
pub fn normalize_email(value: &str) -> String {
    value.trim().to_lowercase()
}

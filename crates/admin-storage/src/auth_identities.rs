//! External identity links (GitHub, Google).

use fleet_core_common::now;
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::users::normalize_email;
use crate::Result;

/// One linked external identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIdentity {
    /// Stable row id.
    pub id: String,
    /// Owning user.
    pub user_id: String,
    /// `github` or `google`.
    pub provider: String,
    /// Provider-side immutable subject id.
    pub subject: String,
    /// Verified email reported by the provider.
    pub email: String,
    /// Provider login or username.
    pub login: Option<String>,
    /// Provider avatar URL.
    pub avatar_url: Option<String>,
    /// When the identity was linked.
    pub created_at: fleet_core_common::Timestamp,
    /// Last successful sign-in.
    pub last_login_at: Option<fleet_core_common::Timestamp>,
}

/// Handle for identity queries.
#[derive(Clone)]
pub struct AuthIdentities {
    pool: SqlitePool,
}

impl AuthIdentities {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, user_id, provider, subject, email, login, avatar_url, \
                       created_at, last_login_at";

impl AuthIdentities {
    /// Link an identity to a user. Returns the existing row when the
    /// `(provider, subject)` pair is already linked to the same user.
    pub async fn link(
        &self,
        user_id: &str,
        provider: &str,
        subject: &str,
        email: &str,
        login: Option<&str>,
        avatar_url: Option<&str>,
    ) -> Result<AuthIdentity> {
        let timestamp = now();
        sqlx::query(
            r#"
            INSERT INTO auth_identities
                (id, user_id, provider, subject, email, login, avatar_url, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(provider, subject) DO UPDATE SET
                email = excluded.email,
                login = excluded.login,
                avatar_url = excluded.avatar_url
            "#,
        )
        .bind(fleet_core_common::new_id("ident"))
        .bind(user_id)
        .bind(provider)
        .bind(subject)
        .bind(normalize_email(email))
        .bind(login)
        .bind(avatar_url)
        .bind(format_ts(timestamp))
        .execute(&self.pool)
        .await?;
        self.find(provider, subject)
            .await?
            .ok_or_else(|| crate::StorageError::InvalidData("identity vanished after link".into()))
    }

    /// Look up an identity by provider subject.
    pub async fn find(&self, provider: &str, subject: &str) -> Result<Option<AuthIdentity>> {
        let row: Option<IdentityRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM auth_identities WHERE provider = ?1 AND subject = ?2"
        ))
        .bind(provider)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(AuthIdentity::from))
    }

    /// Every identity linked to a user.
    pub async fn for_user(&self, user_id: &str) -> Result<Vec<AuthIdentity>> {
        let rows: Vec<IdentityRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM auth_identities WHERE user_id = ?1 ORDER BY created_at"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(AuthIdentity::from).collect())
    }

    /// Record a successful sign-in.
    pub async fn touch(&self, provider: &str, subject: &str) -> Result<()> {
        sqlx::query(
            "UPDATE auth_identities SET last_login_at = ?3 WHERE provider = ?1 AND subject = ?2",
        )
        .bind(provider)
        .bind(subject)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[derive(Debug, sqlx::FromRow)]
struct IdentityRow {
    id: String,
    user_id: String,
    provider: String,
    subject: String,
    email: String,
    login: Option<String>,
    avatar_url: Option<String>,
    created_at: String,
    last_login_at: Option<String>,
}

impl From<IdentityRow> for AuthIdentity {
    fn from(row: IdentityRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            provider: row.provider,
            subject: row.subject,
            email: row.email,
            login: row.login,
            avatar_url: row.avatar_url,
            created_at: parse_ts(&row.created_at),
            last_login_at: parse_opt_ts(row.last_login_at),
        }
    }
}

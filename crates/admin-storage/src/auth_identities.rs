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

/// Outcome of unlinking one external identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnlinkIdentity {
    /// The identity was removed; carries its provider for the audit record.
    Unlinked { provider: String },
    /// No identity with that id belongs to the user.
    NotFound,
    /// Removing it would leave the account with no usable sign-in method.
    LastSignInMethod,
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

    /// Remove one identity from a user.
    ///
    /// The last identity is kept unless the account still has a usable bearer
    /// token, so unlinking cannot lock a user out of their own account. The
    /// count, token check and delete share one immediate transaction, so two
    /// concurrent unlinks cannot both observe two identities.
    pub async fn unlink(&self, user_id: &str, identity_id: &str) -> Result<UnlinkIdentity> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let identity: Option<(String,)> =
            sqlx::query_as("SELECT provider FROM auth_identities WHERE id = ?1 AND user_id = ?2")
                .bind(identity_id)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((provider,)) = identity else {
            tx.rollback().await?;
            return Ok(UnlinkIdentity::NotFound);
        };

        let (identities,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM auth_identities WHERE user_id = ?1")
                .bind(user_id)
                .fetch_one(&mut *tx)
                .await?;
        if identities <= 1 {
            let token: Option<TokenState> = sqlx::query_as(
                "SELECT token_hash, token_issued_at, token_expires_at, disabled_at \
                 FROM users WHERE id = ?1",
            )
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
            let usable = token.is_some_and(|token| {
                token.token_hash.is_some()
                    && token.token_issued_at.is_some()
                    && token.disabled_at.is_none()
                    && token
                        .token_expires_at
                        .is_none_or(|value| parse_ts(&value) > now())
            });
            if !usable {
                tx.rollback().await?;
                return Ok(UnlinkIdentity::LastSignInMethod);
            }
        }

        sqlx::query("DELETE FROM auth_identities WHERE id = ?1")
            .bind(identity_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(UnlinkIdentity::Unlinked { provider })
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

/// Credential state used by the last-sign-in-method guard.
#[derive(Debug, sqlx::FromRow)]
struct TokenState {
    token_hash: Option<String>,
    token_issued_at: Option<String>,
    token_expires_at: Option<String>,
    disabled_at: Option<String>,
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

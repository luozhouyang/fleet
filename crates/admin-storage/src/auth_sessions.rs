//! Browser session storage.
//!
//! Only the SHA-256 hash of the session token is stored. A session is valid
//! while `revoked_at` is unset and `expires_at` is in the future.

use fleet_core_common::{now, Timestamp};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

/// A persisted browser session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    /// Owning user.
    pub user_id: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Absolute expiry.
    pub expires_at: Timestamp,
    /// Last request seen with this session.
    pub last_seen_at: Timestamp,
    /// Client user agent at creation.
    pub user_agent: Option<String>,
    /// Client address at creation.
    pub ip: Option<String>,
    /// Set once revoked.
    pub revoked_at: Option<Timestamp>,
}

/// Handle for session queries.
#[derive(Clone)]
pub struct AuthSessions {
    pool: SqlitePool,
}

impl AuthSessions {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl AuthSessions {
    /// Create a session and return its row.
    pub async fn create(
        &self,
        id_hash: &str,
        user_id: &str,
        expires_at: Timestamp,
        user_agent: Option<&str>,
        ip: Option<&str>,
    ) -> Result<AuthSession> {
        let timestamp = now();
        sqlx::query(
            r#"
            INSERT INTO auth_sessions
                (id_hash, user_id, created_at, expires_at, last_seen_at, user_agent, ip)
            VALUES (?1, ?2, ?3, ?4, ?3, ?5, ?6)
            "#,
        )
        .bind(id_hash)
        .bind(user_id)
        .bind(format_ts(timestamp))
        .bind(format_ts(expires_at))
        .bind(user_agent)
        .bind(ip)
        .execute(&self.pool)
        .await?;
        Ok(AuthSession {
            user_id: user_id.to_string(),
            created_at: timestamp,
            expires_at,
            last_seen_at: timestamp,
            user_agent: user_agent.map(str::to_owned),
            ip: ip.map(str::to_owned),
            revoked_at: None,
        })
    }

    /// Resolve a live session by its token hash.
    pub async fn active(&self, id_hash: &str) -> Result<Option<AuthSession>> {
        let row: Option<SessionRow> = sqlx::query_as(
            r#"
            SELECT user_id, created_at, expires_at, last_seen_at, user_agent, ip, revoked_at
            FROM auth_sessions
            WHERE id_hash = ?1 AND revoked_at IS NULL
            "#,
        )
        .bind(id_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(AuthSession::from).filter(AuthSession::is_live))
    }

    /// Record activity without changing the absolute expiry.
    pub async fn touch(&self, id_hash: &str) -> Result<()> {
        sqlx::query("UPDATE auth_sessions SET last_seen_at = ?2 WHERE id_hash = ?1")
            .bind(id_hash)
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Revoke one session.
    pub async fn revoke(&self, id_hash: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE auth_sessions SET revoked_at = ?2 WHERE id_hash = ?1 AND revoked_at IS NULL",
        )
        .bind(id_hash)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Revoke every session of a user. Used when the account is disabled.
    pub async fn revoke_all_for_user(&self, user_id: &str) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE auth_sessions SET revoked_at = ?2 WHERE user_id = ?1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Delete sessions that expired or were revoked more than `retention` ago.
    pub async fn prune(&self, retention_secs: i64) -> Result<u64> {
        let cutoff = now() - chrono::Duration::seconds(retention_secs.max(0));
        let result = sqlx::query(
            r#"
            DELETE FROM auth_sessions
            WHERE expires_at < ?1 OR (revoked_at IS NOT NULL AND revoked_at < ?1)
            "#,
        )
        .bind(format_ts(cutoff))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

impl AuthSession {
    /// Whether the session may still be used.
    pub fn is_live(&self) -> bool {
        self.revoked_at.is_none() && self.expires_at > now()
    }
}

#[derive(Debug, sqlx::FromRow)]
struct SessionRow {
    user_id: String,
    created_at: String,
    expires_at: String,
    last_seen_at: String,
    user_agent: Option<String>,
    ip: Option<String>,
    revoked_at: Option<String>,
}

impl From<SessionRow> for AuthSession {
    fn from(row: SessionRow) -> Self {
        Self {
            user_id: row.user_id,
            created_at: parse_ts(&row.created_at),
            expires_at: parse_ts(&row.expires_at),
            last_seen_at: parse_ts(&row.last_seen_at),
            user_agent: row.user_agent,
            ip: row.ip,
            revoked_at: parse_opt_ts(row.revoked_at),
        }
    }
}

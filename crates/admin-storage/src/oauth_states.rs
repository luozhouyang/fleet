//! Single-use OAuth authorization state (`state` + PKCE verifier + nonce).

use fleet_core_common::{now, Timestamp};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

/// A pending OAuth authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthState {
    /// `github` or `google`.
    pub provider: String,
    /// PKCE code verifier.
    pub pkce_verifier: String,
    /// OpenID Connect nonce, when the provider returns one.
    pub nonce: Option<String>,
    /// Redirect URI used for the authorization request.
    pub redirect_uri: String,
    /// Expiry.
    pub expires_at: Timestamp,
}

/// Handle for OAuth state queries.
#[derive(Clone)]
pub struct OAuthStates {
    pool: SqlitePool,
}

impl OAuthStates {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl OAuthStates {
    /// Store a new state, keyed by the hash of the opaque `state` value.
    pub async fn create(&self, state_hash: &str, state: &OAuthState) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO auth_oauth_states
                (state_hash, provider, pkce_verifier, nonce, redirect_uri, created_at, expires_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(state_hash)
        .bind(&state.provider)
        .bind(&state.pkce_verifier)
        .bind(&state.nonce)
        .bind(&state.redirect_uri)
        .bind(format_ts(now()))
        .bind(format_ts(state.expires_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically consume a state. Returns `None` when it is unknown, already
    /// consumed or expired.
    pub async fn consume(&self, state_hash: &str) -> Result<Option<OAuthState>> {
        let timestamp = now();
        let row: Option<StateRow> = sqlx::query_as(
            r#"
            UPDATE auth_oauth_states
            SET consumed_at = ?2
            WHERE state_hash = ?1
              AND consumed_at IS NULL
              AND expires_at > ?2
            RETURNING provider, pkce_verifier, nonce, redirect_uri, expires_at
            "#,
        )
        .bind(state_hash)
        .bind(format_ts(timestamp))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(OAuthState::from))
    }

    /// Delete states that are consumed or long expired.
    pub async fn prune(&self) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM auth_oauth_states
            WHERE consumed_at IS NOT NULL OR expires_at < ?1
            "#,
        )
        .bind(format_ts(now() - chrono::Duration::hours(1)))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[derive(Debug, sqlx::FromRow)]
struct StateRow {
    provider: String,
    pkce_verifier: String,
    nonce: Option<String>,
    redirect_uri: String,
    expires_at: String,
}

impl From<StateRow> for OAuthState {
    fn from(row: StateRow) -> Self {
        Self {
            provider: row.provider,
            pkce_verifier: row.pkce_verifier,
            nonce: row.nonce,
            redirect_uri: row.redirect_uri,
            expires_at: parse_ts(&row.expires_at),
        }
    }
}

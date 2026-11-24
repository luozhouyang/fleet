//! Idempotency keys for retried mutation RPCs.
//!
//! A client that retries a mutation sends the same `Idempotency-Key` every
//! time. The first request reserves the key, executes, and caches its response;
//! later requests with the same key replay that response without executing the
//! command again. A reservation that is still running rejects duplicates with
//! `conflict` instead of waiting, and an execution error releases the key so a
//! corrected retry can run.

use std::future::Future;

use chrono::Duration;
use fleet_core_common::{error_codes, now, ApiError};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::helpers::format_ts;
use crate::Result;

/// How long a completed response is replayed for.
pub const COMPLETED_TTL_SECS: i64 = 24 * 60 * 60;

/// How long an in-flight reservation blocks duplicate execution. A daemon
/// crash leaves the reservation behind; it is reclaimed after this lease.
pub const RESERVATION_LEASE_SECS: i64 = 10 * 60;

/// Outcome of reserving an idempotency key.
#[derive(Debug, Clone, PartialEq)]
pub enum Reservation {
    /// The caller owns the key and must execute the request.
    Started,
    /// A completed request with this key exists; replay its response.
    Completed(Value),
    /// Another request with this key is still running.
    InProgress,
    /// The key was already used for a different RPC method.
    MethodMismatch,
}

/// Handle for idempotency-key storage.
#[derive(Clone)]
pub struct IdempotencyKeys {
    pool: SqlitePool,
}

impl IdempotencyKeys {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Reserve `key` for `method`, or report why it cannot be reserved.
    ///
    /// Expired rows are pruned first, including stale reservations left by a
    /// crashed process.
    pub async fn reserve(&self, key: &str, method: &str) -> Result<Reservation> {
        for _ in 0..3 {
            self.purge_expired().await?;
            if let Some((stored_method, response)) = self.lookup(key).await? {
                if stored_method != method {
                    return Ok(Reservation::MethodMismatch);
                }
                return match response {
                    Some(raw) => Ok(Reservation::Completed(serde_json::from_str(&raw)?)),
                    None => Ok(Reservation::InProgress),
                };
            }
            let inserted = sqlx::query(
                "INSERT INTO idempotency_keys (key, method, response, created_at, expires_at) \
                 VALUES (?1, ?2, NULL, ?3, ?4) \
                 ON CONFLICT(key) DO NOTHING",
            )
            .bind(key)
            .bind(method)
            .bind(format_ts(now()))
            .bind(format_ts(now() + Duration::seconds(RESERVATION_LEASE_SECS)))
            .execute(&self.pool)
            .await?;
            if inserted.rows_affected() == 1 {
                return Ok(Reservation::Started);
            }
            // Another writer won the insert race; the next iteration reads it.
        }
        Ok(Reservation::InProgress)
    }

    /// Cache a successful response and extend the key to its full TTL.
    pub async fn complete(&self, key: &str, method: &str, response: &Value) -> Result<()> {
        let raw = serde_json::to_string(response)?;
        sqlx::query(
            "INSERT INTO idempotency_keys (key, method, response, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(key) DO UPDATE SET \
                 method = excluded.method, \
                 response = excluded.response, \
                 expires_at = excluded.expires_at",
        )
        .bind(key)
        .bind(method)
        .bind(raw)
        .bind(format_ts(now()))
        .bind(format_ts(now() + Duration::seconds(COMPLETED_TTL_SECS)))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Release an in-flight reservation after a failed execution.
    ///
    /// A completed row is left untouched so a cached success cannot be lost.
    pub async fn release(&self, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM idempotency_keys WHERE key = ?1 AND response IS NULL")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Execute `action` at most once per key and cache its success.
    pub async fn execute<F, Fut>(
        &self,
        key: &str,
        method: &str,
        action: F,
    ) -> std::result::Result<Value, ApiError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = std::result::Result<Value, ApiError>>,
    {
        match self.reserve(key, method).await.map_err(ApiError::from)? {
            Reservation::Started => {
                let result = action().await;
                match &result {
                    Ok(response) => {
                        if let Err(error) = self.complete(key, method, response).await {
                            // The command already ran; returning success is
                            // safer than asking the client to retry it.
                            tracing::warn!(key = %key, error = %error, "failed to cache idempotent response");
                        }
                    }
                    Err(_) => {
                        if let Err(error) = self.release(key).await {
                            tracing::warn!(key = %key, error = %error, "failed to release idempotency key");
                        }
                    }
                }
                result
            }
            Reservation::Completed(response) => Ok(response),
            Reservation::InProgress => Err(ApiError::new(
                error_codes::CONFLICT,
                "another request with this Idempotency-Key is still running",
            )),
            Reservation::MethodMismatch => Err(ApiError::new(
                error_codes::CONFLICT,
                "this Idempotency-Key was already used for a different operation",
            )),
        }
    }

    async fn lookup(&self, key: &str) -> Result<Option<(String, Option<String>)>> {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT method, response FROM idempotency_keys WHERE key = ?1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn purge_expired(&self) -> Result<()> {
        sqlx::query("DELETE FROM idempotency_keys WHERE expires_at <= ?1")
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Purge expired keys. Exposed for tests and maintenance paths.
    pub async fn prune(&self) -> Result<u64> {
        let result = sqlx::query("DELETE FROM idempotency_keys WHERE expires_at <= ?1")
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

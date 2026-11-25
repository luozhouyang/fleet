//! Single-use WebSocket tickets.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use fleet_core_common::{now, Timestamp};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

/// Create this many tickets between retention sweeps.
const PRUNE_EVERY: u64 = 256;

/// Handle for ticket queries.
#[derive(Clone)]
pub struct RealtimeTickets {
    pool: SqlitePool,
    inserts: Arc<AtomicU64>,
}

impl RealtimeTickets {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            inserts: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl RealtimeTickets {
    /// Store a ticket keyed by the hash of its plaintext value.
    pub async fn create(
        &self,
        ticket_hash: &str,
        user_id: &str,
        org_id: &str,
        session_hash: Option<&str>,
        expires_at: Timestamp,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO realtime_tickets
                (ticket_hash, user_id, org_id, session_hash, created_at, expires_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(ticket_hash)
        .bind(user_id)
        .bind(org_id)
        .bind(session_hash)
        .bind(format_ts(now()))
        .bind(format_ts(expires_at))
        .execute(&self.pool)
        .await?;
        // Every browser reconnect mints a ticket. Sweep periodically instead
        // of letting used and expired rows accumulate for the process lifetime.
        let inserts = self.inserts.fetch_add(1, Ordering::Relaxed) + 1;
        if inserts == 1 || inserts.is_multiple_of(PRUNE_EVERY) {
            self.prune().await?;
        }
        Ok(())
    }

    /// Atomically consume a ticket. Returns the user, organization and the
    /// session that minted it (when the ticket came from a cookie), or `None`
    /// when the ticket is unknown, used or expired.
    pub async fn consume(
        &self,
        ticket_hash: &str,
    ) -> Result<Option<(String, String, Option<String>)>> {
        let timestamp = now();
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            r#"
            UPDATE realtime_tickets
            SET consumed_at = ?2
            WHERE ticket_hash = ?1
              AND consumed_at IS NULL
              AND expires_at > ?2
            RETURNING user_id, org_id, session_hash
            "#,
        )
        .bind(ticket_hash)
        .bind(format_ts(timestamp))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Delete tickets that expired or were consumed more than an hour ago.
    pub async fn prune(&self) -> Result<u64> {
        let cutoff = now() - chrono::Duration::hours(1);
        let result = sqlx::query(
            "DELETE FROM realtime_tickets \
             WHERE (consumed_at IS NOT NULL AND consumed_at < ?1) \
                OR expires_at < ?1",
        )
        .bind(format_ts(cutoff))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

//! Transactional event outbox and consumer cursor.
//!
//! Durable domain events are appended in the same SQLite transaction as their
//! projection rows. A consumer (the admin) reads `pending` events and advances
//! its cursor with `ack`; acknowledged rows are pruned.

use std::sync::Arc;

use fleet_core_common::{now, Timestamp};
use fleet_core_protocol::agent::{AgentEvent, AgentEventType};
use sqlx::{SqliteConnection, SqlitePool};
use tokio::sync::Mutex;

use crate::helpers::*;
use crate::Result;

/// The consumer name used by the agent admin.
pub const ADMIN_CONSUMER: &str = "admin";

/// Backlog snapshot for readiness and metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxStats {
    /// Number of retained (unpruned) rows.
    pub depth: u64,
    /// Creation time of the oldest retained row.
    pub oldest_created_at: Option<Timestamp>,
}

/// One persisted outbox event.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxEntry {
    /// Globally increasing outbox sequence.
    pub sequence: u64,
    /// Full unified event payload.
    pub event: AgentEvent,
    /// When the event was recorded.
    pub created_at: Timestamp,
}

/// Outbox and cursor queries.
#[derive(Clone)]
pub struct EventOutbox {
    pool: SqlitePool,
    /// SQLite DEFERRED transactions can otherwise race and move the cursor
    /// backwards; acknowledgements are serialized within this process.
    ack_lock: Arc<Mutex<()>>,
}

impl EventOutbox {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            ack_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Append one event in its own transaction.
    pub async fn append(&self, event: &AgentEvent) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let sequence = append_on(&mut tx, event).await?;
        tx.commit().await?;
        Ok(sequence)
    }

    /// Pending events after the admin cursor that are not yet acknowledged,
    /// oldest first.
    pub async fn pending(&self, limit: usize) -> Result<Vec<OutboxEntry>> {
        self.pending_filtered(limit, |_| true).await
    }

    /// Pending events selected by `should_decode`, oldest first.
    ///
    /// A replay can acknowledge events out of order. Those rows stay in the
    /// outbox until the gap below them is filled, but they must not consume the
    /// replay window again: only rows without an `event_acks` entry still need
    /// delivery.
    ///
    /// The predicate runs before a row's payload is decoded, so the delivery
    /// loop can skip sequences that are still inside their retry window
    /// without paying the JSON parse cost. Skipped rows are left untouched,
    /// including rows that would otherwise be quarantined as corrupt.
    pub async fn pending_filtered<F>(
        &self,
        limit: usize,
        mut should_decode: F,
    ) -> Result<Vec<OutboxEntry>>
    where
        F: FnMut(u64) -> bool,
    {
        let cursor = self.cursor().await?;
        let limit = limit.clamp(1, 10_000) as i64;
        let rows = sqlx::query_as::<_, OutboxRow>(
            r#"
            SELECT e.sequence, e.event_id, e.payload_json, e.created_at
            FROM event_outbox e
            WHERE e.sequence > ?1
              AND NOT EXISTS (
                  SELECT 1 FROM event_acks a WHERE a.sequence = e.sequence
              )
            ORDER BY e.sequence ASC
            LIMIT ?2
            "#,
        )
        .bind(cursor as i64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            let sequence = u64::try_from(row.sequence).map_err(|_| {
                crate::StorageError::InvalidData(format!(
                    "outbox sequence {} is not a valid u64",
                    row.sequence
                ))
            })?;
            if !should_decode(sequence) {
                continue;
            }
            if row.payload_json.len() > MAX_OUTBOX_PAYLOAD_BYTES {
                tracing::error!(
                    sequence,
                    event_id = %row.event_id,
                    bytes = row.payload_json.len(),
                    "discarding an oversized outbox event"
                );
                self.ack(sequence).await?;
                sqlx::query("DELETE FROM event_outbox WHERE sequence = ?1")
                    .bind(sequence as i64)
                    .execute(&self.pool)
                    .await?;
                continue;
            }
            match serde_json::from_str::<AgentEvent>(&row.payload_json) {
                Ok(event) => entries.push(OutboxEntry {
                    sequence,
                    event,
                    created_at: parse_ts(&row.created_at),
                }),
                Err(error) => {
                    // A corrupt row can never be delivered. Advancing past it
                    // avoids a permanent cursor stall that would also block
                    // every later event and grow the outbox forever.
                    tracing::error!(
                        sequence,
                        event_id = %row.event_id,
                        error = %error,
                        "discarding an unparseable outbox event"
                    );
                    self.ack(sequence).await?;
                    // `ack` retains the sequence in `event_acks` until the
                    // contiguous cursor passes it; the payload itself can go.
                    sqlx::query("DELETE FROM event_outbox WHERE sequence = ?1")
                        .bind(sequence as i64)
                        .execute(&self.pool)
                        .await?;
                }
            }
        }
        Ok(entries)
    }

    /// Current admin cursor.
    pub async fn cursor(&self) -> Result<u64> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT acked_sequence FROM event_cursors WHERE consumer = ?1")
                .bind(ADMIN_CONSUMER)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|(value,)| u64::try_from(value).ok())
            .unwrap_or(0))
    }

    /// Acknowledge one outbox sequence.
    ///
    /// The admin acknowledges each event it applied, so acknowledgements can
    /// arrive out of order after a replay. Acknowledged sequences above a gap
    /// are retained in `event_acks`; the durable cursor only advances over the
    /// contiguous acknowledged prefix and acknowledged rows are pruned then.
    ///
    /// Cursors never move backwards. Returns the effective cursor.
    pub async fn ack(&self, sequence: u64) -> Result<u64> {
        let sequence_i64 = i64::try_from(sequence)
            .map_err(|_| crate::StorageError::InvalidData("sequence overflow".to_owned()))?;
        let _guard = self.ack_lock.lock().await;
        // The cursor lookup reads before the acknowledgement writes; take the
        // write lock up front so a concurrent append cannot invalidate the
        // snapshot and fail the upgrade with SQLITE_BUSY_SNAPSHOT.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = read_cursor_on(&mut tx).await?;
        if sequence_i64 <= current {
            tx.commit().await?;
            return Ok(u64::try_from(current).unwrap_or(0));
        }

        let (maximum,): (i64,) =
            sqlx::query_as("SELECT COALESCE(MAX(sequence), 0) FROM event_outbox")
                .fetch_one(&mut *tx)
                .await?;
        if sequence_i64 > maximum {
            // The row may have been acknowledged and deleted by a session
            // discard while its acknowledgement was still in flight. Accept
            // that replay instead of failing the admin's acknowledgement.
            let already_acked: Option<(i64,)> =
                sqlx::query_as("SELECT sequence FROM event_acks WHERE sequence = ?1")
                    .bind(sequence_i64)
                    .fetch_optional(&mut *tx)
                    .await?;
            if already_acked.is_none() {
                return Err(crate::StorageError::InvalidData(format!(
                    "acknowledged sequence {sequence} is not in the outbox"
                )));
            }
        } else {
            insert_ack_on(&mut tx, sequence_i64).await?;
        }
        let effective = advance_cursor_on(&mut tx).await?;
        tx.commit().await?;
        Ok(u64::try_from(effective).unwrap_or(0))
    }

    /// Acknowledge a batch of applied outbox sequences.
    ///
    /// Batches let a consumer confirm hundreds of events with one request while
    /// preserving the exact-sequence contract: sequences above the incomplete
    /// gap stay acknowledged in `event_acks` until the cursor can pass them.
    /// Sequences that are already pruned are ignored, so a batch racing a
    /// session discard does not fail.
    pub async fn ack_batch(&self, sequences: &[u64]) -> Result<u64> {
        if sequences.is_empty() {
            return self.cursor().await;
        }
        if sequences.len() > MAX_ACK_BATCH {
            return Err(crate::StorageError::InvalidData(format!(
                "acknowledgement batch exceeds {MAX_ACK_BATCH} sequences"
            )));
        }
        let mut sorted = sequences
            .iter()
            .map(|sequence| {
                i64::try_from(*sequence)
                    .map_err(|_| crate::StorageError::InvalidData("sequence overflow".to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        sorted.sort_unstable();
        sorted.dedup();

        let _guard = self.ack_lock.lock().await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = read_cursor_on(&mut tx).await?;
        let (maximum,): (i64,) =
            sqlx::query_as("SELECT COALESCE(MAX(sequence), 0) FROM event_outbox")
                .fetch_one(&mut *tx)
                .await?;
        for sequence in sorted {
            if sequence <= current {
                continue;
            }
            if sequence > maximum {
                tracing::debug!(
                    sequence,
                    maximum,
                    "ignoring acknowledgement for a pruned outbox row"
                );
                continue;
            }
            insert_ack_on(&mut tx, sequence).await?;
        }
        let effective = advance_cursor_on(&mut tx).await?;
        tx.commit().await?;
        Ok(u64::try_from(effective).unwrap_or(0))
    }

    /// Acknowledge and delete pending events for a deleted session that are
    /// older than its `SessionDeleted` sequence.
    ///
    /// Once the admin deletes its session row, it can no longer verify
    /// ownership of these older events and would defer them forever. The
    /// deletion event itself is kept so the admin and browser observe it.
    pub async fn discard_session_before(&self, session_id: &str, sequence: u64) -> Result<()> {
        let _guard = self.ack_lock.lock().await;
        let mut tx = self.pool.begin().await?;
        discard_session_before_on(&mut tx, session_id, sequence).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn lock_acks(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.ack_lock.lock().await
    }

    /// Number of retained outbox rows.
    pub async fn len(&self) -> Result<u64> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM event_outbox")
            .fetch_one(&self.pool)
            .await?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Backlog depth and the age marker of the oldest retained row.
    pub async fn stats(&self) -> Result<OutboxStats> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM event_outbox")
            .fetch_one(&self.pool)
            .await?;
        let oldest: Option<(String,)> =
            sqlx::query_as("SELECT created_at FROM event_outbox ORDER BY sequence LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(OutboxStats {
            depth: u64::try_from(count).unwrap_or(0),
            oldest_created_at: oldest.map(|(created_at,)| parse_ts(&created_at)),
        })
    }

    /// Whether the outbox is empty.
    pub async fn is_empty(&self) -> Result<bool> {
        Ok(self.len().await? == 0)
    }
}

pub(crate) async fn append_on(conn: &mut SqliteConnection, event: &AgentEvent) -> Result<u64> {
    let payload = serde_json::to_string(event)?;
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO event_outbox (
            event_id, session_id, event_type, payload_json, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
    )
    .bind(&event.event_id)
    .bind(&event.session_id)
    .bind(event.event_type.as_str())
    .bind(payload)
    .bind(format_ts(now()))
    .execute(&mut *conn)
    .await?;
    let (sequence,): (i64,) =
        sqlx::query_as("SELECT sequence FROM event_outbox WHERE event_id = ?1")
            .bind(&event.event_id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(u64::try_from(sequence).unwrap_or(0))
}

/// Acknowledge and remove earlier pending events for a deleted session in the
/// caller's transaction, then advance the contiguous cursor.
pub(crate) async fn discard_session_before_on(
    conn: &mut SqliteConnection,
    session_id: &str,
    sequence: u64,
) -> Result<()> {
    let sequence = i64::try_from(sequence)
        .map_err(|_| crate::StorageError::InvalidData("sequence overflow".to_owned()))?;
    let now = format_ts(now());
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO event_acks (sequence, acked_at)
        SELECT sequence, ?2
        FROM event_outbox
        WHERE session_id = ?1 AND sequence < ?3
        "#,
    )
    .bind(session_id)
    .bind(&now)
    .bind(sequence)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM event_outbox WHERE session_id = ?1 AND sequence < ?2")
        .bind(session_id)
        .bind(sequence)
        .execute(&mut *conn)
        .await?;

    advance_cursor_on(conn).await?;
    Ok(())
}

/// Maximum number of sequences accepted by one batch acknowledgement.
pub const MAX_ACK_BATCH: usize = 1024;
/// Hard cap for one serialized outbox payload a consumer will accept.
const MAX_OUTBOX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

async fn read_cursor_on(conn: &mut SqliteConnection) -> Result<i64> {
    let (current,): (i64,) =
        sqlx::query_as("SELECT acked_sequence FROM event_cursors WHERE consumer = ?1")
            .bind(ADMIN_CONSUMER)
            .fetch_one(&mut *conn)
            .await?;
    Ok(current)
}

async fn insert_ack_on(conn: &mut SqliteConnection, sequence: i64) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO event_acks (sequence, acked_at) VALUES (?1, ?2)")
        .bind(sequence)
        .bind(format_ts(now()))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Advance the contiguous cursor, prune delivered rows and stale acks.
///
/// The first outbox row above the cursor without an acknowledgement is the
/// first gap; everything before it is safe to prune. When every remaining row
/// is acknowledged, the cursor moves to the newest outbox or acknowledgement
/// sequence, whichever is higher: an acknowledged row may already have been
/// deleted (a corrupt or session-discarded row), and its acknowledgement must
/// still carry the cursor past it. One query replaces the per-sequence probe
/// that used to make large batches O(n).
async fn advance_cursor_on(conn: &mut SqliteConnection) -> Result<i64> {
    let current = read_cursor_on(conn).await?;
    let (maximum,): (i64,) = sqlx::query_as("SELECT COALESCE(MAX(sequence), 0) FROM event_outbox")
        .fetch_one(&mut *conn)
        .await?;
    let (acked_maximum,): (i64,) =
        sqlx::query_as("SELECT COALESCE(MAX(sequence), 0) FROM event_acks")
            .fetch_one(&mut *conn)
            .await?;
    let gap: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT e.sequence
        FROM event_outbox e
        WHERE e.sequence > ?1
          AND NOT EXISTS (
              SELECT 1 FROM event_acks a WHERE a.sequence = e.sequence
          )
        ORDER BY e.sequence
        LIMIT 1
        "#,
    )
    .bind(current)
    .fetch_optional(&mut *conn)
    .await?;
    let effective = match gap {
        Some((sequence,)) => (sequence - 1).max(current),
        None => maximum.max(acked_maximum).max(current),
    };
    sqlx::query(
        "UPDATE event_cursors SET acked_sequence = MAX(acked_sequence, ?2), updated_at = ?3 \
         WHERE consumer = ?1",
    )
    .bind(ADMIN_CONSUMER)
    .bind(effective)
    .bind(format_ts(now()))
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM event_outbox WHERE sequence <= ?1")
        .bind(effective)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM event_acks WHERE sequence <= ?1")
        .bind(effective)
        .execute(&mut *conn)
        .await?;
    Ok(effective)
}

/// Events that belong in the durable outbox.
///
/// Streaming and observability updates are live-only. `terminal.output` is a
/// chunk stream like a message delta: nothing projects it, so persisting every
/// chunk would add a write transaction and replay traffic per chunk without a
/// recoverable state change. Activity and usage reports are also live-only
/// until a projection consumes them.
pub fn is_durable_event(event_type: AgentEventType) -> bool {
    !event_type.is_live_only()
}

#[derive(sqlx::FromRow)]
struct OutboxRow {
    sequence: i64,
    event_id: String,
    payload_json: String,
    created_at: String,
}

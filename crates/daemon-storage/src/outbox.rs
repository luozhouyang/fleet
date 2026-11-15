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

    /// Pending events after the admin cursor, oldest first.
    pub async fn pending(&self, limit: usize) -> Result<Vec<OutboxEntry>> {
        let cursor = self.cursor().await?;
        let limit = limit.clamp(1, 10_000) as i64;
        let rows = sqlx::query_as::<_, OutboxRow>(
            r#"
            SELECT sequence, event_id, payload_json, created_at
            FROM event_outbox
            WHERE sequence > ?1
            ORDER BY sequence ASC
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
        let mut tx = self.pool.begin().await?;
        let current: (i64,) =
            sqlx::query_as("SELECT acked_sequence FROM event_cursors WHERE consumer = ?1")
                .bind(ADMIN_CONSUMER)
                .fetch_one(&mut *tx)
                .await?;
        if sequence_i64 <= current.0 {
            tx.commit().await?;
            return Ok(u64::try_from(current.0).unwrap_or(0));
        }

        let (maximum,): (i64,) =
            sqlx::query_as("SELECT COALESCE(MAX(sequence), 0) FROM event_outbox")
                .fetch_one(&mut *tx)
                .await?;
        if sequence_i64 > maximum {
            return Err(crate::StorageError::InvalidData(format!(
                "acknowledged sequence {sequence} is not in the outbox"
            )));
        }

        sqlx::query("INSERT OR IGNORE INTO event_acks (sequence, acked_at) VALUES (?1, ?2)")
            .bind(sequence_i64)
            .bind(format_ts(now()))
            .execute(&mut *tx)
            .await?;

        let mut effective = current.0;
        loop {
            let next = effective + 1;
            let exists: Option<(i64,)> =
                sqlx::query_as("SELECT sequence FROM event_acks WHERE sequence = ?1")
                    .bind(next)
                    .fetch_optional(&mut *tx)
                    .await?;
            if exists.is_none() {
                break;
            }
            effective = next;
        }

        sqlx::query(
            r#"
            UPDATE event_cursors
            SET acked_sequence = MAX(event_cursors.acked_sequence, ?2),
                updated_at = ?3
            WHERE consumer = ?1
            "#,
        )
        .bind(ADMIN_CONSUMER)
        .bind(effective)
        .bind(format_ts(now()))
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM event_outbox WHERE sequence <= ?1")
            .bind(effective)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM event_acks WHERE sequence <= ?1")
            .bind(effective)
            .execute(&mut *tx)
            .await?;
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

    let (current,): (i64,) =
        sqlx::query_as("SELECT acked_sequence FROM event_cursors WHERE consumer = ?1")
            .bind(ADMIN_CONSUMER)
            .fetch_one(&mut *conn)
            .await?;
    let mut effective = current;
    loop {
        let next = effective + 1;
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT sequence FROM event_acks WHERE sequence = ?1")
                .bind(next)
                .fetch_optional(&mut *conn)
                .await?;
        if exists.is_none() {
            break;
        }
        effective = next;
    }
    sqlx::query(
        "UPDATE event_cursors SET acked_sequence = MAX(acked_sequence, ?2), updated_at = ?3 \
         WHERE consumer = ?1",
    )
    .bind(ADMIN_CONSUMER)
    .bind(effective)
    .bind(&now)
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
    Ok(())
}

/// Events that belong in the durable outbox.
pub fn is_durable_event(event_type: AgentEventType) -> bool {
    !matches!(
        event_type,
        AgentEventType::MessageStarted
            | AgentEventType::MessageDelta
            | AgentEventType::ToolInputDelta
            | AgentEventType::AgentRawEvent
    )
}

#[derive(sqlx::FromRow)]
struct OutboxRow {
    sequence: i64,
    event_id: String,
    payload_json: String,
    created_at: String,
}

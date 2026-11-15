//! Transactional event outbox and consumer cursor.
//!
//! Durable domain events are appended in the same SQLite transaction as their
//! projection rows. A consumer (the admin) reads `pending` events and advances
//! its cursor with `ack`; acknowledged rows are pruned.

use fleet_core_common::{now, Timestamp};
use fleet_core_protocol::agent::{AgentEvent, AgentEventType};
use sqlx::{SqliteConnection, SqlitePool};

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
}

impl EventOutbox {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
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
            SELECT sequence, payload_json, created_at
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
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let event = serde_json::from_str::<AgentEvent>(&row.payload_json).ok()?;
                Some(OutboxEntry {
                    sequence: u64::try_from(row.sequence).ok()?,
                    event,
                    created_at: parse_ts(&row.created_at),
                })
            })
            .collect())
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

    /// Advance the admin cursor and delete acknowledged rows.
    ///
    /// Cursors never move backwards. Returns the effective cursor.
    pub async fn ack(&self, cursor: u64) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO event_cursors (consumer, acked_sequence, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(consumer) DO UPDATE SET
                acked_sequence = MAX(event_cursors.acked_sequence, excluded.acked_sequence),
                updated_at = excluded.updated_at
            "#,
        )
        .bind(ADMIN_CONSUMER)
        .bind(cursor as i64)
        .bind(format_ts(now()))
        .execute(&mut *tx)
        .await?;
        let effective: (i64,) =
            sqlx::query_as("SELECT acked_sequence FROM event_cursors WHERE consumer = ?1")
                .bind(ADMIN_CONSUMER)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query("DELETE FROM event_outbox WHERE sequence <= ?1")
            .bind(effective.0)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(u64::try_from(effective.0).unwrap_or(0))
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
    payload_json: String,
    created_at: String,
}

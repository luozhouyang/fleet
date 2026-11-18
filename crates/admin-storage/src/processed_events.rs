//! Durable deduplication for applied agent events.
//!
//! The admin process keeps an in-memory set as a hot cache, but a restart
//! must not turn an unacknowledged replay into a second application. This
//! table records each durable event id after its projection was applied; the
//! event pipeline acknowledges the daemon cursor only after that write
//! commits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use fleet_core_common::now;
use sqlx::{SqliteConnection, SqlitePool};

use crate::helpers::format_ts;
use crate::Result;

/// Number of processed event ids retained per machine.
///
/// Replays are bounded by the daemon outbox and its acknowledgement window,
/// so a bounded recent-history window is enough. The in-process cache uses a
/// smaller global window for the hot path.
pub const RETAINED_PER_MACHINE: usize = 32_768;

/// How many inserted ids between retention sweeps.
///
/// Pruning walks the per-machine index, so doing it per event would add a
/// non-trivial cost. The first insert of a process also prunes, which catches
/// anything accumulated while the process was down.
const PRUNE_EVERY: u64 = 4_096;

/// Handle for the processed agent event table.
#[derive(Clone)]
pub struct ProcessedEvents {
    pool: SqlitePool,
    retained: usize,
    inserts: Arc<AtomicU64>,
}

impl ProcessedEvents {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self::with_retention(pool, RETAINED_PER_MACHINE)
    }

    pub(crate) fn with_retention(pool: SqlitePool, retained: usize) -> Self {
        Self {
            pool,
            retained: retained.max(1),
            inserts: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Whether a durable event from `machine_id` was already applied,
    /// including by a previous admin process.
    pub async fn contains(&self, machine_id: &str, event_id: &str) -> Result<bool> {
        let mut conn = self.pool.acquire().await?;
        Self::contains_on(&mut conn, machine_id, event_id).await
    }

    /// Transaction-scoped variant of [`Self::contains`].
    pub(crate) async fn contains_on(
        conn: &mut SqliteConnection,
        machine_id: &str,
        event_id: &str,
    ) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM processed_agent_events WHERE machine_id = ?1 AND event_id = ?2",
        )
        .bind(machine_id)
        .bind(event_id)
        .fetch_optional(&mut *conn)
        .await?;
        Ok(row.is_some())
    }

    /// Record a durable event as applied.
    ///
    /// Returns `false` when the id was already recorded. Callers must only
    /// call this after the event's projection has been applied successfully.
    pub async fn mark(&self, machine_id: &str, event_id: &str) -> Result<bool> {
        let mut conn = self.pool.acquire().await?;
        let inserted = Self::mark_on(&mut conn, machine_id, event_id).await?;
        drop(conn);
        if inserted {
            self.prune_if_due(machine_id).await?;
        }
        Ok(inserted)
    }

    /// Transaction-scoped variant of [`Self::mark`].
    pub(crate) async fn mark_on(
        conn: &mut SqliteConnection,
        machine_id: &str,
        event_id: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO processed_agent_events (
                machine_id, event_id, processed_at
            ) VALUES (?1, ?2, ?3)
            "#,
        )
        .bind(machine_id)
        .bind(event_id)
        .bind(format_ts(now()))
        .execute(&mut *conn)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Run a retention sweep when the amortized threshold is due.
    ///
    /// Retention is maintenance rather than correctness, so the event
    /// transaction commits before pruning.
    pub async fn prune_if_due(&self, machine_id: &str) -> Result<()> {
        let inserts = self.inserts.fetch_add(1, Ordering::Relaxed) + 1;
        if inserts == 1 || inserts.is_multiple_of(PRUNE_EVERY) {
            self.prune(machine_id, self.retained).await?;
        }
        Ok(())
    }

    /// Keep only the newest `keep` event ids for `machine_id`.
    async fn prune(&self, machine_id: &str, keep: usize) -> Result<()> {
        let keep = i64::try_from(keep).unwrap_or(i64::MAX);
        sqlx::query(
            r#"
            DELETE FROM processed_agent_events
            WHERE machine_id = ?1
              AND rowid < (
                  SELECT rowid FROM processed_agent_events
                  WHERE machine_id = ?1
                  ORDER BY rowid DESC
                  LIMIT 1 OFFSET ?2
              )
            "#,
        )
        .bind(machine_id)
        .bind(keep)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AdminStorage;

    async fn storage() -> AdminStorage {
        AdminStorage::connect_in_memory()
            .await
            .expect("in-memory admin storage")
    }

    #[tokio::test]
    async fn marks_are_durable_and_idempotent() {
        let storage = storage().await;
        let events = ProcessedEvents::with_retention(storage.pool().clone(), 32);

        assert!(!events.contains("m_1", "evt_1").await.unwrap());
        assert!(events.mark("m_1", "evt_1").await.unwrap());
        assert!(!events.mark("m_1", "evt_1").await.unwrap());
        assert!(events.contains("m_1", "evt_1").await.unwrap());
        // Another machine's id space is independent even for the same event
        // id, so a reused id cannot suppress another machine's event.
        assert!(!events.contains("m_2", "evt_1").await.unwrap());
        // A different event id is independent.
        assert!(events.mark("m_1", "evt_2").await.unwrap());
        assert!(!events.contains("m_1", "evt_3").await.unwrap());

        // A fresh handle (as after an admin restart) sees the same rows.
        let reopened = ProcessedEvents::with_retention(storage.pool().clone(), 32);
        assert!(reopened.contains("m_1", "evt_1").await.unwrap());
    }

    #[tokio::test]
    async fn pruning_keeps_the_newest_events_per_machine() {
        let storage = storage().await;
        let events = ProcessedEvents::with_retention(storage.pool().clone(), 32);
        for index in 0..6 {
            events.mark("m_1", &format!("evt_{index}")).await.unwrap();
        }
        events.mark("m_2", "other").await.unwrap();

        events.prune("m_1", 3).await.unwrap();
        assert!(!events.contains("m_1", "evt_0").await.unwrap());
        assert!(!events.contains("m_1", "evt_1").await.unwrap());
        assert!(events.contains("m_1", "evt_2").await.unwrap());
        assert!(events.contains("m_1", "evt_3").await.unwrap());
        assert!(events.contains("m_1", "evt_5").await.unwrap());
        // Pruning one machine must not touch another.
        assert!(events.contains("m_2", "other").await.unwrap());
    }
}

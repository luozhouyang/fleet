//! Transaction-scoped table operations.
//!
//! The storage layer does not decide when a unit of work ends. A service (for
//! example the admin event pipeline) calls [`AdminStorage::begin_transaction`],
//! performs table operations through the scoped handles and then commits.
//! Dropping the transaction rolls everything back.
//!
//! This keeps business logic (which tables to touch, which guards to apply)
//! in the service while the SQL and the transaction plumbing stay here.

use fleet_core_protocol::agent::AgentSession;
use sqlx::{Sqlite, SqliteConnection, Transaction};

use crate::processed_events::ProcessedEvents;
use crate::sessions::Sessions;
use crate::{AdminStorage, Result};

impl AdminStorage {
    /// Begin a transaction. The caller owns the scope and must commit it.
    pub async fn begin_transaction(&self) -> Result<AdminTransaction> {
        Ok(AdminTransaction {
            tx: self.pool.begin().await?,
        })
    }
}

/// A unit of work over the admin database.
pub struct AdminTransaction {
    tx: Transaction<'static, Sqlite>,
}

impl AdminTransaction {
    /// Session projection operations bound to this transaction.
    pub fn sessions(&mut self) -> SessionsTransaction<'_> {
        SessionsTransaction { conn: &mut self.tx }
    }

    /// Processed-event deduplication bound to this transaction.
    pub fn processed_events(&mut self) -> ProcessedEventsTransaction<'_> {
        ProcessedEventsTransaction { conn: &mut self.tx }
    }

    /// Commit every operation performed in this transaction.
    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await?;
        Ok(())
    }

    /// Roll back explicitly; dropping the transaction has the same effect.
    pub async fn rollback(self) -> Result<()> {
        self.tx.rollback().await?;
        Ok(())
    }
}

/// Transaction-scoped session projection operations.
pub struct SessionsTransaction<'a> {
    conn: &'a mut SqliteConnection,
}

impl SessionsTransaction<'_> {
    /// Fetch one session by id inside the transaction.
    pub async fn get(&mut self, id: &str) -> Result<Option<AgentSession>> {
        Sessions::get_on(&mut *self.conn, id).await
    }

    /// Insert or update one session inside the transaction.
    pub async fn upsert(&mut self, session: &AgentSession) -> Result<()> {
        Sessions::upsert_on(&mut *self.conn, session).await
    }

    /// Delete one session inside the transaction.
    pub async fn delete(&mut self, id: &str) -> Result<()> {
        Sessions::delete_on(&mut *self.conn, id).await
    }
}

/// Transaction-scoped processed-event operations.
pub struct ProcessedEventsTransaction<'a> {
    conn: &'a mut SqliteConnection,
}

impl ProcessedEventsTransaction<'_> {
    /// Whether this machine already applied the durable event.
    pub async fn contains(&mut self, machine_id: &str, event_id: &str) -> Result<bool> {
        ProcessedEvents::contains_on(&mut *self.conn, machine_id, event_id).await
    }

    /// Record the durable event as applied.
    pub async fn mark(&mut self, machine_id: &str, event_id: &str) -> Result<bool> {
        ProcessedEvents::mark_on(&mut *self.conn, machine_id, event_id).await
    }
}

#[cfg(test)]
mod tests {
    use fleet_core_common::now;

    use super::*;

    async fn storage() -> AdminStorage {
        let storage = AdminStorage::connect_in_memory()
            .await
            .expect("in-memory admin storage");
        // Sessions reference their machine; seed the row the FK requires.
        sqlx::query(
            "INSERT INTO machines (id, name, created_at, updated_at) VALUES ('m_1', 'm_1', ?1, ?1)",
        )
        .bind(crate::helpers::format_ts(now()))
        .execute(storage.pool())
        .await
        .expect("seed machine");
        storage
    }

    fn session(id: &str) -> AgentSession {
        let mut session = AgentSession::new("m_1", "mock", "mock", "/tmp");
        session.id = id.to_string();
        session
    }

    #[tokio::test]
    async fn the_service_controls_the_commit_scope() {
        let storage = storage().await;
        let session = session("s_1");

        let mut tx = storage.begin_transaction().await.unwrap();
        assert!(!tx
            .processed_events()
            .contains("m_1", "evt_1")
            .await
            .unwrap());
        tx.sessions().upsert(&session).await.unwrap();
        assert!(tx.processed_events().mark("m_1", "evt_1").await.unwrap());
        assert!(tx
            .processed_events()
            .contains("m_1", "evt_1")
            .await
            .unwrap());
        tx.commit().await.unwrap();

        assert!(storage.sessions.get("s_1").await.unwrap().is_some());
        assert!(storage
            .processed_events
            .contains("m_1", "evt_1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn dropping_the_transaction_rolls_back_every_table() {
        let storage = storage().await;
        let session = session("s_1");
        {
            let mut tx = storage.begin_transaction().await.unwrap();
            tx.sessions().upsert(&session).await.unwrap();
            tx.processed_events().mark("m_1", "evt_1").await.unwrap();
            // Dropped without commit, as on a crash or a projection error.
        }

        assert!(storage.sessions.get("s_1").await.unwrap().is_none());
        assert!(!storage
            .processed_events
            .contains("m_1", "evt_1")
            .await
            .unwrap());
    }
}

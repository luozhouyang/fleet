//! Messages storage operations.

use fleet_core_protocol::agent::AgentMessage;
use sqlx::{SqliteConnection, SqlitePool};

use crate::helpers::*;
use crate::Result;

use crate::rows::MessageRow;

/// Handle for messages queries.
#[derive(Clone)]
pub struct Messages {
    pool: SqlitePool,
}

impl Messages {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Messages {
    /// Insert or update one message. Message ids are stable across reconnects.
    pub async fn insert(&self, message: &AgentMessage) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.insert_on(&mut conn, message).await
    }

    pub(crate) async fn insert_on(
        &self,
        conn: &mut SqliteConnection,
        message: &AgentMessage,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO agent_messages (id, session_id, turn_id, role, content, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                session_id = excluded.session_id,
                turn_id = excluded.turn_id,
                role = excluded.role,
                content = excluded.content,
                created_at = excluded.created_at
            "#,
        )
        .bind(&message.id)
        .bind(&message.session_id)
        .bind(&message.turn_id)
        .bind(enum_text(&message.role))
        .bind(&message.content)
        .bind(format_ts(message.created_at))
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// List messages for a session, oldest first.
    pub async fn list(&self, session_id: &str) -> Result<Vec<AgentMessage>> {
        let rows = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT id, session_id, turn_id, role, content, created_at
            FROM agent_messages
            WHERE session_id = ?1
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(MessageRow::into_message).collect())
    }
}

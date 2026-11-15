//! Turns storage operations.

use fleet_core_protocol::agent::AgentTurn;
use sqlx::{SqliteConnection, SqlitePool};

use crate::helpers::*;
use crate::Result;

use crate::rows::TurnRow;

/// Handle for turns queries.
#[derive(Clone)]
pub struct Turns {
    pool: SqlitePool,
}

impl Turns {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Turns {
    /// Insert or update one turn.
    pub async fn upsert(&self, turn: &AgentTurn) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.upsert_on(&mut conn, turn).await
    }

    pub(crate) async fn upsert_on(
        &self,
        conn: &mut SqliteConnection,
        turn: &AgentTurn,
    ) -> Result<()> {
        let error_json = match &turn.error {
            Some(error) => Some(serde_json::to_string(error)?),
            None => None,
        };
        sqlx::query(
            r#"
            INSERT INTO agent_turns (id, session_id, status, error_json, started_at, completed_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                session_id = excluded.session_id,
                status = excluded.status,
                error_json = excluded.error_json,
                started_at = excluded.started_at,
                completed_at = excluded.completed_at
            "#,
        )
        .bind(&turn.id)
        .bind(&turn.session_id)
        .bind(enum_text(&turn.status))
        .bind(error_json)
        .bind(format_ts(turn.started_at))
        .bind(format_opt_ts(turn.completed_at))
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// List turns for a session, oldest first.
    pub async fn list(&self, session_id: &str) -> Result<Vec<AgentTurn>> {
        let rows = sqlx::query_as::<_, TurnRow>(
            r#"
            SELECT id, session_id, status, error_json, started_at, completed_at
            FROM agent_turns
            WHERE session_id = ?1
            ORDER BY started_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(TurnRow::into_turn).collect())
    }
}

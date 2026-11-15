//! ToolCalls storage operations.

use fleet_core_common::Timestamp;
use fleet_core_protocol::agent::{AgentToolCall, ToolCallStatus};
use sqlx::{SqliteConnection, SqlitePool};

use crate::helpers::*;
use crate::Result;

use crate::rows::ToolCallRow;

/// Handle for tool_calls queries.
#[derive(Clone)]
pub struct ToolCalls {
    pool: SqlitePool,
}

impl ToolCalls {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl ToolCalls {
    /// Insert or update one structured tool call.
    pub async fn upsert(&self, tool: &AgentToolCall) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.upsert_on(&mut conn, tool).await
    }

    pub(crate) async fn upsert_on(
        &self,
        conn: &mut SqliteConnection,
        tool: &AgentToolCall,
    ) -> Result<()> {
        let input_json = serde_json::to_string(&tool.input)?;
        let output_json = serde_json::to_string(&tool.output)?;
        sqlx::query(
            r#"
            INSERT INTO agent_tool_calls (
                id, session_id, turn_id, name, status, input_json, output_json,
                error, started_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                session_id = excluded.session_id,
                turn_id = COALESCE(excluded.turn_id, agent_tool_calls.turn_id),
                -- Completion payloads often only carry the id and output.
                -- Never replace learned fields with placeholders.
                name = CASE
                    WHEN excluded.name IS NULL
                         OR trim(excluded.name) = ''
                         OR excluded.name = 'tool'
                    THEN agent_tool_calls.name
                    ELSE excluded.name
                END,
                status = excluded.status,
                input_json = CASE
                    WHEN excluded.input_json = 'null' THEN agent_tool_calls.input_json
                    ELSE excluded.input_json
                END,
                output_json = CASE
                    WHEN excluded.output_json = 'null' THEN agent_tool_calls.output_json
                    ELSE excluded.output_json
                END,
                error = COALESCE(excluded.error, agent_tool_calls.error),
                started_at = MIN(agent_tool_calls.started_at, excluded.started_at),
                completed_at = COALESCE(excluded.completed_at, agent_tool_calls.completed_at)
            "#,
        )
        .bind(&tool.id)
        .bind(&tool.session_id)
        .bind(&tool.turn_id)
        .bind(&tool.name)
        .bind(enum_text(&tool.status))
        .bind(input_json)
        .bind(output_json)
        .bind(&tool.error)
        .bind(format_ts(tool.started_at))
        .bind(format_opt_ts(tool.completed_at))
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// List tool calls for a session, oldest first.
    pub async fn list(&self, session_id: &str) -> Result<Vec<AgentToolCall>> {
        let rows = sqlx::query_as::<_, ToolCallRow>(
            r#"
            SELECT id, session_id, turn_id, name, status, input_json, output_json,
                   error, started_at, completed_at
            FROM agent_tool_calls
            WHERE session_id = ?1
            ORDER BY started_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(ToolCallRow::into_tool_call).collect())
    }

    /// Update the terminal state of an existing tool call.
    pub async fn update_status(
        &self,
        session_id: &str,
        tool_call_id: &str,
        status: ToolCallStatus,
        error: Option<&str>,
        completed_at: Option<Timestamp>,
    ) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.update_status_on(
            &mut conn,
            session_id,
            tool_call_id,
            status,
            error,
            completed_at,
        )
        .await
    }

    pub(crate) async fn update_status_on(
        &self,
        conn: &mut SqliteConnection,
        session_id: &str,
        tool_call_id: &str,
        status: ToolCallStatus,
        error: Option<&str>,
        completed_at: Option<Timestamp>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE agent_tool_calls
            SET status = ?3, error = ?4, completed_at = ?5
            WHERE session_id = ?1 AND id = ?2
            "#,
        )
        .bind(session_id)
        .bind(tool_call_id)
        .bind(enum_text(&status))
        .bind(error)
        .bind(completed_at.map(format_ts))
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}

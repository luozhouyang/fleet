//! Sessions storage operations.

use fleet_core_protocol::agent::AgentSession;
use sqlx::{SqliteConnection, SqlitePool};

use crate::helpers::*;
use crate::rows::SessionRow;
use crate::Result;

/// Handle for sessions queries.
#[derive(Clone)]
pub struct Sessions {
    pool: SqlitePool,
}

impl Sessions {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Sessions {
    /// Insert a session or replace its mutable fields.
    pub async fn upsert(&self, s: &AgentSession) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.upsert_on(&mut conn, s).await
    }

    pub(crate) async fn upsert_on(
        &self,
        conn: &mut SqliteConnection,
        s: &AgentSession,
    ) -> Result<()> {
        let last_error_json = match &s.last_error {
            Some(error) => Some(serde_json::to_string(error)?),
            None => None,
        };
        let metadata_json = serde_json::to_string(&s.metadata)?;

        sqlx::query(
            r#"
            INSERT INTO agent_sessions (
                id, machine_id, workspace_id, provider, agent_type, status,
                cwd, title, last_error_json, metadata_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(id) DO UPDATE SET
                machine_id = excluded.machine_id,
                workspace_id = excluded.workspace_id,
                provider = excluded.provider,
                agent_type = excluded.agent_type,
                status = excluded.status,
                cwd = excluded.cwd,
                title = excluded.title,
                last_error_json = excluded.last_error_json,
                metadata_json = excluded.metadata_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&s.id)
        .bind(&s.machine_id)
        .bind(&s.workspace_id)
        .bind(&s.provider)
        .bind(&s.agent_type)
        .bind(enum_text(&s.status))
        .bind(&s.cwd)
        .bind(&s.title)
        .bind(last_error_json)
        .bind(metadata_json)
        .bind(format_ts(s.created_at))
        .bind(format_ts(s.updated_at))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Fetch one session by id.
    pub async fn get(&self, id: &str) -> Result<Option<AgentSession>> {
        let mut conn = self.pool.acquire().await?;
        self.get_on(&mut conn, id).await
    }

    pub(crate) async fn get_on(
        &self,
        conn: &mut SqliteConnection,
        id: &str,
    ) -> Result<Option<AgentSession>> {
        let row = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, machine_id, workspace_id, provider, agent_type, status,
                   cwd, title, last_error_json, metadata_json, created_at, updated_at
            FROM agent_sessions
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;

        Ok(row.map(SessionRow::into_session))
    }

    /// List sessions, optionally restricted to one machine, newest first.
    pub async fn list(&self, machine_id: Option<&str>) -> Result<Vec<AgentSession>> {
        let rows = match machine_id {
            Some(machine_id) => {
                sqlx::query_as::<_, SessionRow>(
                    r#"
                    SELECT id, machine_id, workspace_id, provider, agent_type, status,
                           cwd, title, last_error_json, metadata_json, created_at, updated_at
                    FROM agent_sessions
                    WHERE machine_id = ?1
                    ORDER BY created_at DESC, id DESC
                    "#,
                )
                .bind(machine_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, SessionRow>(
                    r#"
                    SELECT id, machine_id, workspace_id, provider, agent_type, status,
                           cwd, title, last_error_json, metadata_json, created_at, updated_at
                    FROM agent_sessions
                    ORDER BY created_at DESC, id DESC
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows.into_iter().map(SessionRow::into_session).collect())
    }

    /// Sessions in a workspace that can still make progress (or be resumed).
    pub async fn list_active_for_workspace(&self, workspace_id: &str) -> Result<Vec<AgentSession>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, machine_id, workspace_id, provider, agent_type, status,
                   cwd, title, last_error_json, metadata_json, created_at, updated_at
            FROM agent_sessions
            WHERE workspace_id = ?1
            "#,
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(SessionRow::into_session)
            .filter(|session| session.status.is_active())
            .collect())
    }

    /// Delete a session and all of its history.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.delete_on(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn delete_on(&self, conn: &mut SqliteConnection, id: &str) -> Result<()> {
        for sql in [
            "DELETE FROM permission_requests WHERE session_id = ?1",
            "DELETE FROM agent_tool_calls WHERE session_id = ?1",
            "DELETE FROM agent_messages WHERE session_id = ?1",
            "DELETE FROM agent_turns WHERE session_id = ?1",
            "DELETE FROM agent_sessions WHERE id = ?1",
        ] {
            sqlx::query(sql).bind(id).execute(&mut *conn).await?;
        }
        Ok(())
    }
}

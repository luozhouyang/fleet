//! Sessions storage operations.

use std::collections::HashMap;

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
        Self::upsert_on(&mut conn, s).await
    }

    /// Transaction-scoped variant of [`Self::upsert`].
    pub(crate) async fn upsert_on(conn: &mut SqliteConnection, s: &AgentSession) -> Result<()> {
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
                title = CASE
                    WHEN agent_sessions.title_override = 1 THEN agent_sessions.title
                    ELSE excluded.title
                END,
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

    /// Set or clear the operator-facing session title.
    ///
    /// A non-empty title locks the value so later daemon snapshots cannot
    /// overwrite it. Clearing the title unlocks it again.
    pub async fn update_title(&self, id: &str, title: Option<&str>) -> Result<bool> {
        let result =
            sqlx::query("UPDATE agent_sessions SET title = ?2, title_override = ?3 WHERE id = ?1")
                .bind(id)
                .bind(title)
                .bind(i64::from(title.is_some()))
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Operator-set titles for one machine, keyed by session id.
    pub async fn overridden_titles(&self, machine_id: &str) -> Result<HashMap<String, String>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            r#"
            SELECT id, title
            FROM agent_sessions
            WHERE machine_id = ?1 AND title_override = 1 AND title IS NOT NULL
            "#,
        )
        .bind(machine_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    /// Fetch one session by id.
    pub async fn get(&self, id: &str) -> Result<Option<AgentSession>> {
        let mut conn = self.pool.acquire().await?;
        Self::get_on(&mut conn, id).await
    }

    /// Transaction-scoped variant of [`Self::get`].
    pub(crate) async fn get_on(
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

    /// Sessions for a machine that can still make progress (or be resumed).
    ///
    /// Filters in SQL so a disconnect does not deserialize the machine's
    /// entire session history just to find the active rows.
    pub async fn list_active(&self, machine_id: &str) -> Result<Vec<AgentSession>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, machine_id, workspace_id, provider, agent_type, status,
                   cwd, title, last_error_json, metadata_json, created_at, updated_at
            FROM agent_sessions
            WHERE machine_id = ?1
              AND status IN (
                  'creating', 'starting', 'running', 'waiting_for_input',
                  'waiting_for_permission', 'waiting_for_question'
              )
            ORDER BY created_at DESC, id DESC
            "#,
        )
        .bind(machine_id)
        .fetch_all(&self.pool)
        .await?;
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

    /// Delete admin session metadata.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM agent_sessions WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Transaction-scoped variant of [`Self::delete`].
    pub(crate) async fn delete_on(conn: &mut SqliteConnection, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM agent_sessions WHERE id = ?1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }
}

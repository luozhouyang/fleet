//! Agent plan storage operations.
//!
//! Plans replace each other rather than accumulating, so the table keeps one
//! row per session: the latest plan the agent published. The transcript uses
//! it to restore the plan row after a reload.

use fleet_core_protocol::agent::{AgentPlan, PlanEntry};
use sqlx::{SqliteConnection, SqlitePool};

use crate::helpers::*;
use crate::Result;

/// Handle for plan queries.
#[derive(Clone)]
pub struct Plans {
    pool: SqlitePool,
}

impl Plans {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert or replace the latest plan for a session.
    pub async fn upsert(&self, plan: &AgentPlan) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.upsert_on(&mut conn, plan).await
    }

    pub(crate) async fn upsert_on(
        &self,
        conn: &mut SqliteConnection,
        plan: &AgentPlan,
    ) -> Result<()> {
        let entries_json = serde_json::to_string(&plan.entries)?;
        let turn_id = known_turn(conn, &plan.session_id, plan.turn_id.as_deref()).await?;
        sqlx::query(
            r#"
            INSERT INTO agent_plans (
                session_id, id, turn_id, native_plan_id, entries_json, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(session_id) DO UPDATE SET
                id = excluded.id,
                turn_id = excluded.turn_id,
                native_plan_id = excluded.native_plan_id,
                entries_json = excluded.entries_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&plan.session_id)
        .bind(&plan.id)
        .bind(turn_id)
        .bind(&plan.native_plan_id)
        .bind(entries_json)
        .bind(format_ts(plan.updated_at))
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Fetch the latest plan for a session.
    pub async fn get(&self, session_id: &str) -> Result<Option<AgentPlan>> {
        let row: Option<PlanRow> = sqlx::query_as(
            r#"
            SELECT session_id, id, turn_id, native_plan_id, entries_json, updated_at
            FROM agent_plans
            WHERE session_id = ?1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(PlanRow::into_plan))
    }
}

#[derive(sqlx::FromRow)]
struct PlanRow {
    session_id: String,
    id: String,
    turn_id: Option<String>,
    native_plan_id: Option<String>,
    entries_json: String,
    updated_at: String,
}

impl PlanRow {
    fn into_plan(self) -> AgentPlan {
        let entries: Vec<PlanEntry> = serde_json::from_str(&self.entries_json).unwrap_or_default();
        AgentPlan {
            id: self.id,
            session_id: self.session_id,
            turn_id: self.turn_id,
            native_plan_id: self.native_plan_id,
            entries,
            updated_at: parse_ts(&self.updated_at),
        }
    }
}

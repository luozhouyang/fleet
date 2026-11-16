//! User question storage operations.

use fleet_core_common::now;
use fleet_core_protocol::agent::{AgentQuestionRequest, QuestionStatus};
use sqlx::{SqliteConnection, SqlitePool};

use crate::helpers::*;
use crate::rows::QuestionRow;
use crate::Result;

/// Handle for question queries.
#[derive(Clone)]
pub struct Questions {
    pool: SqlitePool,
}

impl Questions {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Questions {
    /// Insert a question request or replace its mutable fields.
    pub async fn upsert(&self, q: &AgentQuestionRequest) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.upsert_on(&mut conn, q).await
    }

    pub(crate) async fn upsert_on(
        &self,
        conn: &mut SqliteConnection,
        q: &AgentQuestionRequest,
    ) -> Result<()> {
        let questions_json = serde_json::to_string(&q.questions)?;
        sqlx::query(
            r#"
            INSERT INTO question_requests (
                id, session_id, turn_id, tool_call_id, questions_json, answers_json,
                status, created_at, resolved_at, expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                session_id = excluded.session_id,
                turn_id = excluded.turn_id,
                tool_call_id = excluded.tool_call_id,
                questions_json = excluded.questions_json,
                answers_json = excluded.answers_json,
                status = excluded.status,
                resolved_at = COALESCE(excluded.resolved_at, question_requests.resolved_at),
                expires_at = excluded.expires_at
            WHERE question_requests.status = 'pending' OR excluded.status != 'pending'
            "#,
        )
        .bind(&q.id)
        .bind(&q.session_id)
        .bind(&q.turn_id)
        .bind(&q.tool_call_id)
        .bind(questions_json)
        .bind(serde_json::to_string(&q.answers)?)
        .bind(enum_text(&q.status))
        .bind(format_ts(q.created_at))
        .bind(format_opt_ts(q.resolved_at))
        .bind(format_opt_ts(q.expires_at))
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Fetch one question request by id.
    pub async fn get(&self, id: &str) -> Result<Option<AgentQuestionRequest>> {
        let row = sqlx::query_as::<_, QuestionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, questions_json, answers_json,
                   status, created_at, resolved_at, expires_at
            FROM question_requests
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(QuestionRow::into_question))
    }

    /// List every question request for a session, oldest first.
    pub async fn list(&self, session_id: &str) -> Result<Vec<AgentQuestionRequest>> {
        let rows = sqlx::query_as::<_, QuestionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, questions_json, answers_json,
                   status, created_at, resolved_at, expires_at
            FROM question_requests
            WHERE session_id = ?1
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(QuestionRow::into_question).collect())
    }

    /// List only pending question requests for a session, oldest first.
    pub async fn list_pending(&self, session_id: &str) -> Result<Vec<AgentQuestionRequest>> {
        let rows = sqlx::query_as::<_, QuestionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, questions_json, answers_json,
                   status, created_at, resolved_at, expires_at
            FROM question_requests
            WHERE session_id = ?1 AND status = ?2
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .bind(enum_text(&QuestionStatus::Pending))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(QuestionRow::into_question).collect())
    }

    /// Resolve a pending question. Returns false when it was not pending.
    pub async fn resolve_pending(&self, id: &str, status: QuestionStatus) -> Result<bool> {
        let resolved_at = now();
        let result = sqlx::query(
            "UPDATE question_requests SET status = ?2, resolved_at = ?3 \
             WHERE id = ?1 AND status = 'pending'",
        )
        .bind(id)
        .bind(enum_text(&status))
        .bind(format_ts(resolved_at))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DaemonStorage;
    use fleet_core_protocol::agent::{AgentQuestion, AgentQuestionOption};

    fn question() -> AgentQuestionRequest {
        AgentQuestionRequest {
            id: "question_1".into(),
            session_id: "session_1".into(),
            turn_id: None,
            tool_call_id: None,
            questions: vec![AgentQuestion {
                id: "q1".into(),
                header: Some("Choose".into()),
                question: "Proceed?".into(),
                options: vec![AgentQuestionOption {
                    label: "yes".into(),
                    description: None,
                    value: None,
                }],
                multi_select: false,
                allow_custom: true,
                is_secret: false,
            }],
            answers: Vec::new(),
            status: QuestionStatus::Pending,
            created_at: now(),
            resolved_at: None,
            expires_at: None,
        }
    }

    #[tokio::test]
    async fn upsert_list_and_resolve_pending_question() {
        let storage = DaemonStorage::connect_in_memory().await.unwrap();
        storage
            .sessions
            .upsert(&fleet_core_protocol::agent::AgentSession::new(
                "machine_1",
                "mock",
                "mock",
                "/tmp",
            ))
            .await
            .unwrap();
        let mut q = question();
        q.session_id = storage.sessions.list(None).await.unwrap()[0].id.clone();
        storage.questions.upsert(&q).await.unwrap();
        assert_eq!(
            storage.questions.list(&q.session_id).await.unwrap(),
            vec![q.clone()]
        );
        assert!(storage
            .questions
            .resolve_pending(&q.id, QuestionStatus::Answered)
            .await
            .unwrap());
        assert!(!storage
            .questions
            .resolve_pending(&q.id, QuestionStatus::Rejected)
            .await
            .unwrap());
        assert_eq!(
            storage.questions.get(&q.id).await.unwrap().unwrap().status,
            QuestionStatus::Answered
        );
    }

    #[tokio::test]
    async fn stale_pending_upsert_cannot_reopen_a_resolved_question() {
        let storage = DaemonStorage::connect_in_memory().await.unwrap();
        storage
            .sessions
            .upsert(&fleet_core_protocol::agent::AgentSession::new(
                "machine_1",
                "mock",
                "mock",
                "/tmp",
            ))
            .await
            .unwrap();
        let mut q = question();
        q.session_id = storage.sessions.list(None).await.unwrap()[0].id.clone();
        storage.questions.upsert(&q).await.unwrap();
        assert!(storage
            .questions
            .resolve_pending(&q.id, QuestionStatus::Answered)
            .await
            .unwrap());

        let mut stale = q.clone();
        stale.answers = vec![fleet_core_protocol::agent::AgentQuestionAnswer {
            id: "q1".into(),
            selected: vec!["yes".into()],
            custom: None,
        }];
        storage.questions.upsert(&stale).await.unwrap();

        let stored = storage.questions.get(&q.id).await.unwrap().unwrap();
        assert_eq!(stored.status, QuestionStatus::Answered);
        assert!(stored.answers.is_empty(), "stale write must not win");

        stale.status = QuestionStatus::Answered;
        storage.questions.upsert(&stale).await.unwrap();
        let stored = storage.questions.get(&q.id).await.unwrap().unwrap();
        assert_eq!(stored.answers.len(), 1);
        assert!(stored.resolved_at.is_some(), "resolved_at must not regress");
    }
}

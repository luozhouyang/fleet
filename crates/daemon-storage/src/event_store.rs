//! Atomic application of durable agent events to the daemon projection and
//! outbox.
//!
//! The runtime feeds every emitted event here. Durable events update their
//! projection rows and append an outbox entry in one SQLite transaction;
//! ephemeral deltas are ignored.

use fleet_core_common::{now, ApiError};
use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentMessage, AgentPermissionRequest, AgentQuestionRequest,
    AgentSession, AgentToolCall, AgentTurn, MessageCompletedPayload, PermissionEventPayload,
    PermissionStatus, QuestionEventPayload, QuestionStatus, SessionErrorPayload, SessionStatus,
    StatusChangedPayload, ToolCallStatus, ToolEventPayload, ToolStatusPayload, TurnStatus,
};
use sqlx::{SqliteConnection, SqliteTransaction};

use crate::helpers::*;
use crate::outbox::{append_on, discard_session_before_on, is_durable_event};
use crate::DaemonStorage;
use crate::Result;

impl DaemonStorage {
    /// Apply one durable event to the projection and outbox atomically.
    ///
    /// Returns the assigned outbox cursor for durable events, or `None` for
    /// ephemeral events and events for a session that no longer exists.
    pub async fn record_event(&self, event: &AgentEvent) -> Result<Option<u64>> {
        if !is_durable_event(event.event_type) {
            return Ok(None);
        }
        // The session-existence check reads before the projection writes. A
        // deferred transaction could then fail to upgrade with
        // SQLITE_BUSY_SNAPSHOT if another connection commits in between, so
        // take the write lock up front and let the busy timeout queue it.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        // Events queued before a session deletion must not resurrect its
        // projection rows or notify the consumer about it. A creation belongs
        // to a session the runtime persisted before emitting it; if the row is
        // already gone, the start failed and the queued creation must be
        // dropped rather than recreate a ghost on the admin.
        if event.event_type != AgentEventType::SessionDeleted
            && !session_exists(&mut tx, &event.session_id).await?
        {
            tx.rollback().await?;
            return Ok(None);
        }
        apply_projection(&mut tx, event).await?;
        let cursor = append_on(&mut tx, event).await?;
        tx.commit().await?;
        Ok(Some(cursor))
    }

    /// Record a failed session start as `session.error` + `session.deleted`.
    ///
    /// A provider that fails to start must not leave a session row behind, but
    /// deleting the row directly and emitting the deletion event later leaves
    /// a crash window with a ghost session. Persist the error and the deletion
    /// in one transaction so the projection, the outbox and the repair path
    /// agree even if the daemon dies immediately afterwards.
    pub async fn record_session_start_failure(
        &self,
        error_event: &AgentEvent,
        deleted_event: &AgentEvent,
    ) -> Result<(u64, u64)> {
        if error_event.event_type != AgentEventType::SessionError
            || deleted_event.event_type != AgentEventType::SessionDeleted
        {
            return Err(crate::StorageError::InvalidData(
                "record_session_start_failure requires session.error and session.deleted"
                    .to_owned(),
            ));
        }
        if error_event.session_id != deleted_event.session_id {
            return Err(crate::StorageError::InvalidData(
                "start failure events belong to different sessions".to_owned(),
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        apply_projection(&mut tx, error_event).await?;
        let error_cursor = append_on(&mut tx, error_event).await?;
        apply_projection(&mut tx, deleted_event).await?;
        let deleted_cursor = append_on(&mut tx, deleted_event).await?;
        tx.commit().await?;
        Ok((error_cursor, deleted_cursor))
    }

    /// Delete a session, append `SessionDeleted`, and discard its earlier
    /// pending outbox events in one transaction.
    ///
    /// This is the ACID variant used by session deletion. Doing the three
    /// operations separately could leave a deleted session whose pending
    /// events permanently block the admin's contiguous acknowledgement cursor.
    pub async fn record_session_deleted(&self, event: &AgentEvent) -> Result<u64> {
        if event.event_type != AgentEventType::SessionDeleted {
            return Err(crate::StorageError::InvalidData(
                "record_session_deleted requires a session.deleted event".to_owned(),
            ));
        }
        let _guard = self.outbox.lock_acks().await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        apply_projection(&mut tx, event).await?;
        let sequence = append_on(&mut tx, event).await?;
        discard_session_before_on(&mut tx, &event.session_id, sequence).await?;
        tx.commit().await?;
        Ok(sequence)
    }
}

async fn apply_projection(tx: &mut SqliteTransaction<'_>, event: &AgentEvent) -> Result<()> {
    match event.event_type {
        AgentEventType::SessionCreated => {
            if let Some(session) = parse_nested::<AgentSession>(&event.payload, "session") {
                self::sessions(tx, &session).await?;
            }
        }
        AgentEventType::SessionStarted
        | AgentEventType::SessionResumed
        | AgentEventType::SessionTerminated => {
            let status = match event.event_type {
                AgentEventType::SessionTerminated => SessionStatus::Terminated,
                _ => SessionStatus::Running,
            };
            update_session_status(tx, &event.session_id, status).await?;
        }
        AgentEventType::SessionDeleted => {
            delete_session(tx, &event.session_id).await?;
        }
        AgentEventType::SessionError => {
            if let Ok(payload) =
                serde_json::from_value::<SessionErrorPayload>(event.payload.clone())
            {
                if let Some(mut session) = load_session(tx, &event.session_id).await? {
                    session.last_error = Some(payload.error);
                    if session.status.can_transition_to(SessionStatus::Failed) {
                        session.status = SessionStatus::Failed;
                    }
                    session.updated_at = now();
                    sessions(tx, &session).await?;
                }
            }
        }
        AgentEventType::AgentStatusChanged => {
            if let Ok(payload) =
                serde_json::from_value::<StatusChangedPayload>(event.payload.clone())
            {
                update_session_status(tx, &event.session_id, payload.status).await?;
            }
        }
        AgentEventType::TurnStarted => {
            let turn = parse_nested::<AgentTurn>(&event.payload, "turn").or_else(|| {
                event.turn_id.as_ref().map(|turn_id| AgentTurn {
                    id: turn_id.clone(),
                    session_id: event.session_id.clone(),
                    status: TurnStatus::Running,
                    started_at: event.timestamp,
                    completed_at: None,
                    error: None,
                })
            });
            if let Some(turn) = turn {
                self::turns(tx, &turn).await?;
            }
        }
        AgentEventType::TurnCompleted
        | AgentEventType::TurnInterrupted
        | AgentEventType::TurnFailed => {
            if let Some(turn_id) = event.turn_id.as_deref() {
                let status = match event.event_type {
                    AgentEventType::TurnCompleted => TurnStatus::Completed,
                    AgentEventType::TurnInterrupted => TurnStatus::Interrupted,
                    _ => TurnStatus::Failed,
                };
                let error = payload_error(&event.payload);
                self::turn_status(tx, turn_id, status, error.as_ref()).await?;
            }
        }
        AgentEventType::MessageCompleted => {
            if let Some(mut message) = event
                .payload
                .get("message")
                .and_then(|value| serde_json::from_value::<AgentMessage>(value.clone()).ok())
                .or_else(|| {
                    serde_json::from_value::<MessageCompletedPayload>(event.payload.clone())
                        .ok()
                        .map(|payload| payload.message)
                })
                .or_else(|| serde_json::from_value::<AgentMessage>(event.payload.clone()).ok())
            {
                if message.session_id.is_empty() {
                    message.session_id = event.session_id.clone();
                }
                self::message(tx, &message).await?;
            }
        }
        AgentEventType::ToolStarted | AgentEventType::ToolCompleted => {
            if let Some(tool) = parse_tool(&event.payload) {
                self::tool(tx, &tool).await?;
            }
        }
        AgentEventType::ToolFailed => {
            if let Some(tool) = parse_tool(&event.payload) {
                self::tool(tx, &tool).await?;
            } else if let Ok(status) =
                serde_json::from_value::<ToolStatusPayload>(event.payload.clone())
            {
                self::tool_status(
                    tx,
                    &event.session_id,
                    &status.tool_call_id,
                    status.status,
                    status.error.as_deref(),
                )
                .await?;
            }
        }
        AgentEventType::PermissionRequested
        | AgentEventType::PermissionApproved
        | AgentEventType::PermissionRejected => {
            if let Some(mut permission) = event
                .payload
                .get("permission")
                .and_then(|value| {
                    serde_json::from_value::<AgentPermissionRequest>(value.clone()).ok()
                })
                .or_else(|| {
                    serde_json::from_value::<PermissionEventPayload>(event.payload.clone())
                        .ok()
                        .map(|payload| payload.permission)
                })
                .or_else(|| {
                    serde_json::from_value::<AgentPermissionRequest>(event.payload.clone()).ok()
                })
            {
                match event.event_type {
                    AgentEventType::PermissionApproved => {
                        permission.status = PermissionStatus::Approved;
                        permission.resolved_at.get_or_insert(event.timestamp);
                    }
                    AgentEventType::PermissionRejected => {
                        // A local cancellation/timeout carries its terminal
                        // status in the payload; only an actual decision is
                        // forced to `Rejected`.
                        if permission.status == PermissionStatus::Pending {
                            permission.status = PermissionStatus::Rejected;
                        }
                        permission.resolved_at.get_or_insert(event.timestamp);
                    }
                    _ => {}
                }
                self::permission(tx, &permission).await?;
            }
        }
        AgentEventType::QuestionRequested
        | AgentEventType::QuestionAnswered
        | AgentEventType::QuestionRejected => {
            if let Some(mut question) = event
                .payload
                .get("question")
                .and_then(|value| {
                    serde_json::from_value::<AgentQuestionRequest>(value.clone()).ok()
                })
                .or_else(|| {
                    serde_json::from_value::<QuestionEventPayload>(event.payload.clone())
                        .ok()
                        .map(|payload| payload.question)
                })
                .or_else(|| {
                    serde_json::from_value::<AgentQuestionRequest>(event.payload.clone()).ok()
                })
            {
                match event.event_type {
                    AgentEventType::QuestionAnswered => {
                        question.status = QuestionStatus::Answered;
                        question.resolved_at.get_or_insert(event.timestamp);
                    }
                    AgentEventType::QuestionRejected => {
                        if question.status == QuestionStatus::Pending {
                            question.status = QuestionStatus::Rejected;
                        }
                        question.resolved_at.get_or_insert(event.timestamp);
                    }
                    _ => {}
                }
                self::question(tx, &question).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_nested<T: serde::de::DeserializeOwned>(
    payload: &serde_json::Value,
    key: &str,
) -> Option<T> {
    payload
        .get(key)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or_else(|| serde_json::from_value(payload.clone()).ok())
}

fn parse_tool(payload: &serde_json::Value) -> Option<AgentToolCall> {
    payload
        .get("tool")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or_else(|| {
            serde_json::from_value::<ToolEventPayload>(payload.clone())
                .ok()
                .map(|payload| payload.tool)
        })
        .or_else(|| serde_json::from_value(payload.clone()).ok())
}

fn payload_error(payload: &serde_json::Value) -> Option<ApiError> {
    payload
        .get("error")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

async fn session_exists(tx: &mut SqliteConnection, session_id: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM agent_sessions WHERE id = ?1")
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;
    Ok(row.is_some())
}

/// Resolve an optional turn reference, dropping it when the row is unknown.
///
/// Agents can name turns and tool calls in their own id space; a dangling
/// reference must become `NULL` instead of failing the foreign key (and
/// therefore stalling the durable event). The migration that introduced the
/// foreign keys applied the same cleanup to historical rows.
async fn known_turn(
    tx: &mut SqliteConnection,
    session_id: &str,
    turn_id: Option<&str>,
) -> Result<Option<String>> {
    let Some(turn_id) = turn_id else {
        return Ok(None);
    };
    let row: Option<(String,)> =
        sqlx::query_as("SELECT id FROM agent_turns WHERE id = ?1 AND session_id = ?2")
            .bind(turn_id)
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
    Ok(row.map(|(id,)| id))
}

/// Resolve an optional tool-call reference, dropping unknown ids.
async fn known_tool_call(
    tx: &mut SqliteConnection,
    session_id: &str,
    tool_call_id: Option<&str>,
) -> Result<Option<String>> {
    let Some(tool_call_id) = tool_call_id else {
        return Ok(None);
    };
    let row: Option<(String,)> =
        sqlx::query_as("SELECT id FROM agent_tool_calls WHERE id = ?1 AND session_id = ?2")
            .bind(tool_call_id)
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
    Ok(row.map(|(id,)| id))
}

async fn load_session(tx: &mut SqliteConnection, session_id: &str) -> Result<Option<AgentSession>> {
    sqlx::query_as::<_, crate::rows::SessionRow>(
        r#"
        SELECT id, machine_id, workspace_id, provider, agent_type, status,
               cwd, title, last_error_json, metadata_json, created_at, updated_at
        FROM agent_sessions WHERE id = ?1
        "#,
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await
    .map(|row| row.map(crate::rows::SessionRow::into_session))
    .map_err(Into::into)
}

async fn sessions(tx: &mut SqliteConnection, session: &AgentSession) -> Result<()> {
    let last_error_json = match &session.last_error {
        Some(error) => Some(serde_json::to_string(error)?),
        None => None,
    };
    let metadata_json = serde_json::to_string(&session.metadata)?;
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
    .bind(&session.id)
    .bind(&session.machine_id)
    .bind(&session.workspace_id)
    .bind(&session.provider)
    .bind(&session.agent_type)
    .bind(enum_text(&session.status))
    .bind(&session.cwd)
    .bind(&session.title)
    .bind(last_error_json)
    .bind(metadata_json)
    .bind(format_ts(session.created_at))
    .bind(format_ts(session.updated_at))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn update_session_status(
    tx: &mut SqliteConnection,
    session_id: &str,
    status: SessionStatus,
) -> Result<()> {
    sqlx::query("UPDATE agent_sessions SET status = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(session_id)
        .bind(enum_text(&status))
        .bind(format_ts(now()))
        .execute(&mut *tx)
        .await?;
    Ok(())
}

async fn delete_session(tx: &mut SqliteConnection, session_id: &str) -> Result<()> {
    for sql in [
        "DELETE FROM question_requests WHERE session_id = ?1",
        "DELETE FROM permission_requests WHERE session_id = ?1",
        "DELETE FROM agent_tool_calls WHERE session_id = ?1",
        "DELETE FROM agent_messages WHERE session_id = ?1",
        "DELETE FROM agent_turns WHERE session_id = ?1",
        "DELETE FROM agent_sessions WHERE id = ?1",
    ] {
        sqlx::query(sql).bind(session_id).execute(&mut *tx).await?;
    }
    Ok(())
}

async fn turns(tx: &mut SqliteConnection, turn: &AgentTurn) -> Result<()> {
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
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn turn_status(
    tx: &mut SqliteConnection,
    turn_id: &str,
    status: TurnStatus,
    error: Option<&ApiError>,
) -> Result<()> {
    let error_json = match error {
        Some(error) => Some(serde_json::to_string(error)?),
        None => None,
    };
    sqlx::query(
        "UPDATE agent_turns SET status = ?2, error_json = ?3, completed_at = ?4 WHERE id = ?1",
    )
    .bind(turn_id)
    .bind(enum_text(&status))
    .bind(error_json)
    .bind(format_ts(now()))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn message(tx: &mut SqliteConnection, message: &AgentMessage) -> Result<()> {
    let turn_id = known_turn(tx, &message.session_id, message.turn_id.as_deref()).await?;
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
    .bind(turn_id)
    .bind(enum_text(&message.role))
    .bind(&message.content)
    .bind(format_ts(message.created_at))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn tool(tx: &mut SqliteConnection, tool: &AgentToolCall) -> Result<()> {
    let input_json = serde_json::to_string(&tool.input)?;
    let output_json = serde_json::to_string(&tool.output)?;
    let turn_id = known_turn(tx, &tool.session_id, tool.turn_id.as_deref()).await?;
    sqlx::query(
        r#"
        INSERT INTO agent_tool_calls (
            id, session_id, turn_id, name, status, input_json, output_json,
            error, started_at, completed_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(id) DO UPDATE SET
            session_id = excluded.session_id,
            turn_id = COALESCE(excluded.turn_id, agent_tool_calls.turn_id),
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
    .bind(turn_id)
    .bind(&tool.name)
    .bind(enum_text(&tool.status))
    .bind(input_json)
    .bind(output_json)
    .bind(&tool.error)
    .bind(format_ts(tool.started_at))
    .bind(format_opt_ts(tool.completed_at))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn tool_status(
    tx: &mut SqliteConnection,
    session_id: &str,
    tool_call_id: &str,
    status: ToolCallStatus,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE agent_tool_calls SET status = ?3, error = ?4, completed_at = ?5 \
         WHERE session_id = ?1 AND id = ?2",
    )
    .bind(session_id)
    .bind(tool_call_id)
    .bind(enum_text(&status))
    .bind(error)
    .bind(format_ts(now()))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn permission(tx: &mut SqliteConnection, permission: &AgentPermissionRequest) -> Result<()> {
    let input_json = serde_json::to_string(&permission.input)?;
    let turn_id = known_turn(tx, &permission.session_id, permission.turn_id.as_deref()).await?;
    let tool_call_id = known_tool_call(
        tx,
        &permission.session_id,
        permission.tool_call_id.as_deref(),
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO permission_requests (
            id, session_id, turn_id, tool_call_id, tool_name, description,
            input_json, options_json, status, created_at, resolved_at, expires_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(id) DO UPDATE SET
            session_id = excluded.session_id,
            turn_id = excluded.turn_id,
            tool_call_id = excluded.tool_call_id,
            tool_name = excluded.tool_name,
            description = excluded.description,
            input_json = excluded.input_json,
            options_json = excluded.options_json,
            status = excluded.status,
            resolved_at = excluded.resolved_at,
            expires_at = excluded.expires_at
        "#,
    )
    .bind(&permission.id)
    .bind(&permission.session_id)
    .bind(turn_id)
    .bind(tool_call_id)
    .bind(&permission.tool_name)
    .bind(&permission.description)
    .bind(input_json)
    .bind(serde_json::to_string(&permission.options)?)
    .bind(enum_text(&permission.status))
    .bind(format_ts(permission.created_at))
    .bind(format_opt_ts(permission.resolved_at))
    .bind(format_opt_ts(permission.expires_at))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn question(tx: &mut SqliteConnection, question: &AgentQuestionRequest) -> Result<()> {
    let turn_id = known_turn(tx, &question.session_id, question.turn_id.as_deref()).await?;
    let tool_call_id =
        known_tool_call(tx, &question.session_id, question.tool_call_id.as_deref()).await?;
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
    .bind(&question.id)
    .bind(&question.session_id)
    .bind(turn_id)
    .bind(tool_call_id)
    .bind(serde_json::to_string(&question.questions)?)
    .bind(serde_json::to_string(&question.answers)?)
    .bind(enum_text(&question.status))
    .bind(format_ts(question.created_at))
    .bind(format_opt_ts(question.resolved_at))
    .bind(format_opt_ts(question.expires_at))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

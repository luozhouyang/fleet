//! Agent session and permission proxying.

use std::collections::HashMap;

use fleet_core_common::{error_codes, now};
use fleet_core_protocol::agent::{
    AgentPermissionRequest, AgentQuestionAnswer, AgentQuestionRequest, AgentSession,
    ListQuestionsRequest, RespondPermissionRequest, RespondQuestionRequest, SendMessageRequest,
    SessionStatus, UpdateSessionRequest,
};
use fleet_core_protocol::rpc::{
    methods, AgentRequest, CreateSessionRpcRequest, ListPermissionsRequest, SendMessageRpcRequest,
    SessionDetail, SessionRequest,
};
use futures::future::join_all;

use crate::error::AppError;
use crate::services::workspace_service::WorkspaceService;
use crate::services::{forward, not_found, parse_response, to_payload};
use crate::state::AppState;

/// Agent session operations exposed by the agent admin.
pub struct AgentService;

impl AgentService {
    /// Merge persisted sessions with each online machine's live list.
    ///
    /// Daemon lists are best effort: an unreachable daemon must not make the
    /// dashboard fail, so failures are logged and persisted rows are kept.
    /// `include_closed` controls whether archived (`terminated`) sessions are
    /// returned; active/historical sessions are included by default.
    pub async fn list_sessions(
        state: &AppState,
        include_closed: bool,
    ) -> Result<Vec<AgentSession>, AppError> {
        let mut merged: HashMap<String, AgentSession> = HashMap::new();
        for session in state.db.sessions.list(None).await? {
            merged.insert(session.id.clone(), session);
        }

        let timeout = state.request_timeout();
        let online_machines: std::collections::HashSet<String> = state
            .connections
            .online()
            .into_iter()
            .map(|info| info.machine_id)
            .collect();
        let requests = state.connections.online().into_iter().map(|info| {
            let connections = state.connections.clone();
            async move {
                let result = connections
                    .request_with_timeout(
                        &info.machine_id,
                        methods::SESSIONS_LIST,
                        serde_json::json!({}),
                        timeout,
                    )
                    .await;
                (info.machine_id, result)
            }
        });

        for (machine_id, result) in join_all(requests).await {
            match result {
                Ok(value) => match serde_json::from_value::<Vec<AgentSession>>(value) {
                    Ok(sessions) => {
                        let live_ids: std::collections::HashSet<String> =
                            sessions.iter().map(|session| session.id.clone()).collect();
                        let overridden_titles = state
                            .db
                            .sessions
                            .overridden_titles(&machine_id)
                            .await
                            .unwrap_or_default();
                        for mut session in sessions {
                            if let Some(title) = overridden_titles.get(&session.id) {
                                session.title = Some(title.clone());
                            }
                            if session.metadata.is_object() {
                                if let Some(metadata) = session.metadata.as_object_mut() {
                                    metadata.remove("machine_offline");
                                }
                            }
                            if let Err(err) = state.db.sessions.upsert(&session).await {
                                tracing::warn!(
                                    machine = %machine_id,
                                    session = %session.id,
                                    error = %err,
                                    "failed to persist live session"
                                );
                            }
                            merged.insert(session.id.clone(), session);
                        }
                        // A daemon that no longer reports an active session
                        // must not leave the dashboard showing it as running.
                        if let Ok(stored) = state.db.sessions.list(Some(&machine_id)).await {
                            for mut session in stored {
                                if live_ids.contains(&session.id) || !session.status.is_active() {
                                    continue;
                                }
                                session.status = SessionStatus::Interrupted;
                                session.last_error = Some(fleet_core_common::ApiError::new(
                                    "session_not_reported",
                                    "the machine no longer reports this session; it was interrupted",
                                ));
                                if !session.metadata.is_object() {
                                    session.metadata = serde_json::json!({});
                                }
                                session.metadata["orphaned"] = serde_json::json!(true);
                                session.updated_at = fleet_core_common::now();
                                if let Err(err) = state.db.sessions.upsert(&session).await {
                                    tracing::warn!(session = %session.id, error = %err, "failed to reconcile stale session");
                                }
                                merged.insert(session.id.clone(), session);
                            }
                        }
                    }
                    Err(err) => tracing::warn!(
                        machine = %machine_id,
                        error = %err,
                        "invalid session list from daemon"
                    ),
                },
                Err(err) => tracing::debug!(
                    machine = %machine_id,
                    code = %err.code,
                    "could not list sessions on machine"
                ),
            }
        }

        // A machine that is offline cannot still be running a provider. Mark
        // its active sessions interrupted so the UI and workspace deletion do
        // not depend on a daemon that may never come back. If it reconnects and
        // reports the session as running, the live upsert above restores it.
        let mut offline_updates = Vec::new();
        for session in merged.values_mut() {
            if session.status.is_active() && !online_machines.contains(&session.machine_id) {
                session.status = SessionStatus::Interrupted;
                session.last_error = Some(fleet_core_common::ApiError::new(
                    "machine_offline",
                    "the machine is offline; this session is read-only until it reconnects",
                ));
                if !session.metadata.is_object() {
                    session.metadata = serde_json::json!({});
                }
                session.metadata["machine_offline"] = serde_json::json!(true);
                session.updated_at = now();
                offline_updates.push(session.clone());
            }
        }
        for session in offline_updates {
            if let Err(err) = state.db.sessions.upsert(&session).await {
                tracing::warn!(session = %session.id, error = %err, "failed to reconcile offline session");
            }
        }

        let mut sessions: Vec<AgentSession> = merged
            .into_values()
            .filter(|session| include_closed || session.status != SessionStatus::Terminated)
            .collect();
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(sessions)
    }

    /// Create a session on a connected machine and persist its metadata.
    pub async fn create_session(
        state: &AppState,
        request: CreateSessionRpcRequest,
    ) -> Result<AgentSession, AppError> {
        if request.machine_id.trim().is_empty() {
            return Err(AppError::bad_request("machine_id is required"));
        }
        // Fail fast with machine_offline before touching workspace state.
        crate::services::ensure_online(state, &request.machine_id)?;

        let mut create = request.create;
        if let Some(workspace_id) = create.workspace_id.clone() {
            let workspace = WorkspaceService::get(state, &workspace_id).await?;
            if workspace.machine_id != request.machine_id {
                return Err(AppError::bad_request(
                    "workspace does not belong to the requested machine",
                ));
            }
            if create.cwd.is_none() {
                create.cwd = Some(workspace.root_path);
            }
        }

        let payload = to_payload(&create)?;
        let value = forward(
            state,
            &request.machine_id,
            methods::SESSIONS_CREATE,
            payload,
        )
        .await?;
        let session: AgentSession = parse_response(value, "agent.sessions.create")?;
        state.db.sessions.upsert(&session).await?;
        Ok(session)
    }

    /// Agent catalog for one connected machine.
    ///
    /// The catalog is live-only: a definition's availability depends on the
    /// machine's executables and drivers, which the persisted provider
    /// snapshot cannot describe.
    pub async fn list_agents(
        state: &AppState,
        machine_id: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        crate::services::ensure_online(state, machine_id)?;
        let value = forward(
            state,
            machine_id,
            methods::AGENTS_LIST,
            serde_json::json!({}),
        )
        .await?;
        parse_response(value, "agent.agents.list")
    }

    /// Driver catalog for one connected machine.
    pub async fn list_drivers(
        state: &AppState,
        machine_id: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        crate::services::ensure_online(state, machine_id)?;
        let value = forward(
            state,
            machine_id,
            methods::DRIVERS_LIST,
            serde_json::json!({}),
        )
        .await?;
        parse_response(value, "agent.drivers.list")
    }

    /// Probe one agent definition without creating a session.
    pub async fn probe_agent(
        state: &AppState,
        machine_id: &str,
        agent_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        crate::services::ensure_online(state, machine_id)?;
        let payload = to_payload(&AgentRequest {
            agent_id: agent_id.to_string(),
        })?;
        forward(state, machine_id, methods::AGENTS_PROBE, payload).await
    }

    /// Fetch a session detail, falling back to stored metadata when offline.
    pub async fn get_session(state: &AppState, id: &str) -> Result<SessionDetail, AppError> {
        let session = state
            .db
            .sessions
            .get(id)
            .await?
            .ok_or_else(|| not_found("session", id))?;

        if state.connections.is_online(&session.machine_id) {
            let payload = to_payload(&SessionRequest {
                session_id: id.to_string(),
            })?;
            match forward(state, &session.machine_id, methods::SESSIONS_GET, payload).await {
                Ok(value) => {
                    let mut detail: SessionDetail = parse_response(value, "agent.sessions.get")?;
                    // Transcript, permissions and questions stay on the daemon.
                    state.db.sessions.upsert(&detail.session).await?;
                    // A daemon snapshot carries the provider's title; the
                    // stored row may hold an operator override.
                    if let Ok(Some(stored)) = state.db.sessions.get(id).await {
                        detail.session.title = stored.title;
                    }
                    if !detail.session.metadata.is_object() {
                        detail.session.metadata = serde_json::json!({});
                    }
                    detail.session.metadata["history_unavailable"] = serde_json::json!(false);
                    return Ok(detail);
                }
                Err(err) if err.error.code == error_codes::MACHINE_OFFLINE => {
                    tracing::debug!(session = %id, "machine went offline; using stored session");
                }
                Err(err) if err.error.code == error_codes::NOT_FOUND => {
                    // The daemon no longer knows this session (for example it
                    // restarted without restoring history). Keep the metadata
                    // visible instead of turning it into a 404.
                    tracing::debug!(session = %id, "daemon no longer reports session; using stored metadata");
                }
                Err(err) => return Err(err),
            }
        }

        let mut session = session;
        if session.status.is_active() {
            session.status = SessionStatus::Interrupted;
            session.last_error = Some(fleet_core_common::ApiError::new(
                "session_unavailable",
                "the session is no longer available on its machine",
            ));
            session.updated_at = fleet_core_common::now();
        }
        if !session.metadata.is_object() {
            session.metadata = serde_json::json!({});
        }
        // The daemon owns the transcript. When it cannot be reached, return
        // metadata and an explicit flag instead of a partial cached history.
        session.metadata["history_unavailable"] = serde_json::json!(true);
        let _ = state.db.sessions.upsert(&session).await;
        Ok(SessionDetail {
            session,
            turns: Vec::new(),
            messages: Vec::new(),
            tool_calls: Vec::new(),
            permissions: Vec::new(),
            questions: Vec::new(),
        })
    }

    /// Send a user message to a running session.
    pub async fn send_message(
        state: &AppState,
        id: &str,
        message: SendMessageRequest,
    ) -> Result<serde_json::Value, AppError> {
        let session = Self::session_or_404(state, id).await?;
        let payload = to_payload(&SendMessageRpcRequest {
            session_id: id.to_string(),
            message,
        })?;
        forward(
            state,
            &session.machine_id,
            methods::SESSIONS_SEND_MESSAGE,
            payload,
        )
        .await
    }

    /// Interrupt a running turn.
    pub async fn interrupt(state: &AppState, id: &str) -> Result<serde_json::Value, AppError> {
        Self::session_command(state, id, methods::SESSIONS_INTERRUPT).await
    }

    /// Resume an interrupted session.
    pub async fn resume(state: &AppState, id: &str) -> Result<serde_json::Value, AppError> {
        Self::session_command(state, id, methods::SESSIONS_RESUME).await
    }

    /// Close/archive a session: stop the provider, keep metadata and history.
    pub async fn close(state: &AppState, id: &str) -> Result<serde_json::Value, AppError> {
        let mut session = Self::session_or_404(state, id).await?;
        let payload = to_payload(&SessionRequest {
            session_id: id.to_string(),
        })?;
        let value = forward(
            state,
            &session.machine_id,
            methods::SESSIONS_SHUTDOWN,
            payload,
        )
        .await?;
        session.status = SessionStatus::Terminated;
        session.updated_at = fleet_core_common::now();
        state.db.sessions.upsert(&session).await?;
        Ok(value)
    }

    /// Rename a session. An empty or missing title clears the override and
    /// lets the provider's title apply again.
    pub async fn update_session(
        state: &AppState,
        id: &str,
        request: UpdateSessionRequest,
    ) -> Result<AgentSession, AppError> {
        let title = request
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string);
        if let Some(title) = &title {
            if title.chars().count() > 200 {
                return Err(AppError::bad_request(
                    "session title must be 200 characters or fewer",
                ));
            }
        }
        if !state.db.sessions.update_title(id, title.as_deref()).await? {
            return Err(not_found("session", id));
        }
        state
            .db
            .sessions
            .get(id)
            .await?
            .ok_or_else(|| not_found("session", id))
    }

    /// Permanently delete a session and all of its history.
    pub async fn delete(state: &AppState, id: &str) -> Result<serde_json::Value, AppError> {
        let session = Self::session_or_404(state, id).await?;
        let payload = to_payload(&SessionRequest {
            session_id: id.to_string(),
        })?;
        let value = forward(
            state,
            &session.machine_id,
            methods::SESSIONS_DELETE,
            payload,
        )
        .await?;
        state.event_index.remove_session_machine(id);
        state.db.sessions.delete(id).await?;
        Ok(value)
    }

    /// List permission requests for a session.
    pub async fn list_permissions(
        state: &AppState,
        id: &str,
    ) -> Result<Vec<AgentPermissionRequest>, AppError> {
        let session = Self::session_or_404(state, id).await?;
        if state.connections.is_online(&session.machine_id) {
            let payload = to_payload(&ListPermissionsRequest {
                session_id: id.to_string(),
            })?;
            match forward(
                state,
                &session.machine_id,
                methods::PERMISSIONS_LIST,
                payload,
            )
            .await
            {
                Ok(value) => return parse_response(value, "agent.permissions.list"),
                Err(err) if err.error.code == error_codes::MACHINE_OFFLINE => {
                    tracing::debug!(session = %id, "machine offline; permissions are live-only");
                }
                Err(err) if err.error.code == error_codes::NOT_FOUND => {
                    tracing::debug!(session = %id, "daemon no longer reports session");
                }
                Err(err) => return Err(err),
            }
        }
        Ok(Vec::new())
    }

    /// List user questions for a session.
    pub async fn list_questions(
        state: &AppState,
        id: &str,
    ) -> Result<Vec<AgentQuestionRequest>, AppError> {
        let session = Self::session_or_404(state, id).await?;
        if state.connections.is_online(&session.machine_id) {
            let payload = to_payload(&ListQuestionsRequest {
                session_id: id.to_string(),
            })?;
            match forward(state, &session.machine_id, methods::QUESTIONS_LIST, payload).await {
                Ok(value) => return parse_response(value, "agent.questions.list"),
                Err(err) if err.error.code == error_codes::MACHINE_OFFLINE => {
                    tracing::debug!(session = %id, "machine offline; questions are live-only");
                }
                Err(err) if err.error.code == error_codes::NOT_FOUND => {
                    tracing::debug!(session = %id, "daemon no longer reports session");
                }
                Err(err) => return Err(err),
            }
        }
        Ok(Vec::new())
    }

    /// Approve or reject a permission request through its session's machine.
    pub async fn respond_permission(
        state: &AppState,
        session_id: &str,
        permission_id: &str,
        approve: bool,
        option_id: Option<String>,
        message: Option<String>,
    ) -> Result<serde_json::Value, AppError> {
        let machine_id = Self::machine_for_session(state, session_id).await?;
        let payload = to_payload(&RespondPermissionRequest {
            permission_id: permission_id.to_string(),
            session_id: Some(session_id.to_string()),
            approve,
            option_id,
            message,
        })?;
        forward(state, &machine_id, methods::PERMISSIONS_RESPOND, payload).await
    }

    /// Answer or reject a user question through its session's machine.
    pub async fn respond_question(
        state: &AppState,
        session_id: &str,
        question_id: &str,
        answers: Option<Vec<AgentQuestionAnswer>>,
        reject: bool,
        message: Option<String>,
    ) -> Result<serde_json::Value, AppError> {
        let machine_id = Self::machine_for_session(state, session_id).await?;
        let payload = to_payload(&RespondQuestionRequest {
            question_id: question_id.to_string(),
            session_id: Some(session_id.to_string()),
            answers,
            reject,
            message,
        })?;
        forward(state, &machine_id, methods::QUESTIONS_RESPOND, payload).await
    }

    async fn machine_for_session(state: &AppState, session_id: &str) -> Result<String, AppError> {
        if let Some(machine_id) = state.event_index.session_machine(session_id) {
            return Ok(machine_id);
        }
        let session = state
            .db
            .sessions
            .get(session_id)
            .await?
            .ok_or_else(|| not_found("session", session_id))?;
        state
            .event_index
            .set_session_machine(session_id, &session.machine_id);
        Ok(session.machine_id)
    }

    async fn session_command(
        state: &AppState,
        id: &str,
        method: &str,
    ) -> Result<serde_json::Value, AppError> {
        let session = Self::session_or_404(state, id).await?;
        let payload = to_payload(&SessionRequest {
            session_id: id.to_string(),
        })?;
        forward(state, &session.machine_id, method, payload).await
    }

    async fn session_or_404(state: &AppState, id: &str) -> Result<AgentSession, AppError> {
        state
            .db
            .sessions
            .get(id)
            .await?
            .ok_or_else(|| not_found("session", id))
    }
}

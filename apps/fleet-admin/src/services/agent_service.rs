//! Agent session and permission proxying.

use std::collections::HashMap;

use axum::http::StatusCode;
use fleet_core_common::{error_codes, now};
use fleet_core_protocol::agent::{
    AgentPermissionRequest, AgentSession, PermissionStatus, RespondPermissionRequest,
    SendMessageRequest, SessionStatus,
};
use fleet_core_protocol::rpc::{
    methods, CreateSessionRpcRequest, ListPermissionsRequest, SendMessageRpcRequest, SessionDetail,
    SessionRequest,
};
use futures::future::join_all;

use crate::error::AppError;
use crate::services::event_service::PermissionRoute;
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
                        for mut session in sessions {
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
                create.cwd = Some(workspace.root_path.clone());
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
                    // Transcript rows stay on the daemon. The admin only
                    // mirrors session metadata and permission routing state.
                    state.db.sessions.upsert(&detail.session).await?;
                    for permission in &detail.permissions {
                        if let Err(err) = state.db.permissions.upsert(permission).await {
                            tracing::warn!(session = %id, permission = %permission.id, error = %err, "failed to persist permission");
                        }
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
        let permissions = state.db.permissions.list(id).await.unwrap_or_default();
        let _ = state.db.sessions.upsert(&session).await;
        Ok(SessionDetail {
            session,
            turns: Vec::new(),
            messages: Vec::new(),
            tool_calls: Vec::new(),
            permissions,
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
                Ok(value) => {
                    let permissions: Vec<AgentPermissionRequest> =
                        parse_response(value, "agent.permissions.list")?;
                    for permission in &permissions {
                        if let Err(err) = state.db.permissions.upsert(permission).await {
                            tracing::warn!(
                                permission = %permission.id,
                                error = %err,
                                "failed to persist permission"
                            );
                        }
                    }
                    return Ok(permissions);
                }
                Err(err) if err.error.code == error_codes::MACHINE_OFFLINE => {
                    tracing::debug!(session = %id, "machine offline; using stored permissions");
                }
                Err(err) if err.error.code == error_codes::NOT_FOUND => {
                    // The daemon no longer knows the session; its metadata
                    // can still be visible while stored permissions remain.
                    tracing::debug!(session = %id, "daemon no longer reports session; using stored permissions");
                }
                Err(err) => return Err(err),
            }
        }

        Ok(state.db.permissions.list(id).await?)
    }

    /// Approve or reject a permission request.
    ///
    /// The permission is routed to its machine through the persisted row, or
    /// through the event-built index when no row exists yet.
    pub async fn respond_permission(
        state: &AppState,
        id: &str,
        approve: bool,
        message: Option<String>,
    ) -> Result<serde_json::Value, AppError> {
        let permission = state.db.permissions.get(id).await?;
        if let Some(stored) = permission.as_ref() {
            if stored.status.is_terminal() {
                return Err(AppError::new(
                    StatusCode::CONFLICT,
                    error_codes::CONFLICT,
                    format!("permission {id} is already {:?}", stored.status),
                ));
            }
        }
        let route = match permission.as_ref() {
            Some(stored) => match state.db.sessions.get(&stored.session_id).await? {
                Some(session) => Some(PermissionRoute {
                    session_id: stored.session_id.clone(),
                    machine_id: session.machine_id,
                }),
                None => state.event_index.permission_route(id),
            },
            None => state.event_index.permission_route(id),
        };
        let Some(route) = route else {
            return Err(not_found("permission", id));
        };

        // Claim the pending row before forwarding so two concurrent approvers
        // cannot both reach the provider.
        let next_status = if approve {
            PermissionStatus::Approved
        } else {
            PermissionStatus::Rejected
        };
        if permission.is_some()
            && !state
                .db
                .permissions
                .resolve_pending(id, next_status)
                .await?
        {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                error_codes::CONFLICT,
                format!("permission {id} was already resolved"),
            ));
        }

        let payload = to_payload(&RespondPermissionRequest {
            permission_id: id.to_string(),
            approve,
            message,
        })?;
        if let Err(error) = forward(
            state,
            &route.machine_id,
            methods::PERMISSIONS_RESPOND,
            payload,
        )
        .await
        {
            if permission.is_some() {
                if let Err(revert) = state.db.permissions.revert_pending(id).await {
                    tracing::warn!(permission = %id, error = %revert, "failed to revert permission claim");
                }
            }
            return Err(error);
        }

        match permission {
            Some(mut stored) => {
                stored.status = next_status;
                stored.resolved_at = Some(now());
                state.db.permissions.upsert(&stored).await?;
                Ok(serde_json::to_value(stored).map_err(AppError::internal)?)
            }
            None => Ok(serde_json::json!({
                "ok": true,
                "permission_id": id,
                "status": if approve { "approved" } else { "rejected" },
            })),
        }
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

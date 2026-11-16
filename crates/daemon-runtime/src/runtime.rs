//! Unified runtime: session registry, lifecycle state machine, event dispatch
//! and provider selection.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use fleet_core_common::{now, ApiError};
use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentMessage, AgentPermissionRequest, AgentQuestionRequest,
    AgentSession, AgentStartConfig, AgentToolCall, AgentTurn, MessageCompletedPayload, MessageRole,
    PermissionDecision, PermissionEventPayload, PermissionStatus, ProviderInfo, QuestionDecision,
    QuestionEventPayload, QuestionStatus, SendMessageRequest, SessionErrorPayload, SessionStatus,
    StatusChangedPayload, ToolCallStatus, ToolStatusPayload, TurnStatus,
};
use fleet_core_protocol::rpc::SessionDetail;
use fleet_daemon_storage::DaemonStorage;
use parking_lot::{Mutex, RwLock};
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use crate::error::{Result, RuntimeError};
use crate::event::{
    AgentEventEmitter, PendingPermission, PendingQuestion, PermissionMap, ProcessedAgentEvent,
    QuestionMap,
};
use crate::provider::{AgentProvider, ProviderContext, ProviderRegistry};
use crate::session::SessionHandle;

type StorageHandle = Arc<RwLock<Option<Arc<DaemonStorage>>>>;

/// Unified agent runtime.
pub struct AgentRuntime {
    registry: ProviderRegistry,
    sessions: Arc<DashMap<String, Arc<SessionHandle>>>,
    events: broadcast::Sender<AgentEvent>,
    dispatch: mpsc::UnboundedSender<AgentEvent>,
    processed: broadcast::Sender<ProcessedAgentEvent>,
    storage: StorageHandle,
}

impl AgentRuntime {
    /// Create a runtime from a provider registry.
    ///
    /// `event_capacity` is the broadcast buffer size; values below one are
    /// clamped to one.
    pub fn new(registry: ProviderRegistry, event_capacity: usize) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        let (processed, _) = broadcast::channel(event_capacity.max(1));
        let (dispatch, dispatch_receiver) = mpsc::unbounded_channel();
        let sessions: Arc<DashMap<String, Arc<SessionHandle>>> = Arc::new(DashMap::new());
        let storage: StorageHandle = Arc::new(RwLock::new(None));

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let dispatch_sessions = Arc::clone(&sessions);
            let dispatch_storage = Arc::clone(&storage);
            let dispatch_processed = processed.clone();
            handle.spawn(dispatch_events(
                dispatch_sessions,
                dispatch_storage,
                dispatch_processed,
                dispatch_receiver,
            ));
        } else {
            drop(dispatch_receiver);
            tracing::warn!(
                "AgentRuntime created outside a Tokio runtime; event state tracking is inactive"
            );
        }

        Self {
            registry,
            sessions,
            events,
            dispatch,
            processed,
            storage,
        }
    }

    /// Attach optional persistence. Sessions and permissions are then upserted
    /// as the runtime observes lifecycle and permission events.
    pub fn with_storage(self, storage: Arc<DaemonStorage>) -> Self {
        *self.storage.write() = Some(storage);
        self
    }

    /// Restore persistent session metadata and history after a daemon restart.
    ///
    /// Active sessions become read-only `Interrupted` records: their provider
    /// process is gone, but the transcript remains available. Terminal
    /// sessions are restored as-is. Restored sessions are marked with
    /// `metadata.restored = true` and reject `send_message`/`resume`.
    pub async fn restore_sessions(&self, machine_id: &str) -> Result<()> {
        let Some(database) = self.storage.read().clone() else {
            return Ok(());
        };
        if let Err(error) = database
            .sessions
            .close_stale_children_for_inactive(machine_id)
            .await
        {
            tracing::warn!(machine_id, error = %error, "failed to close stale transcript rows");
        }
        let sessions = database.sessions.list(Some(machine_id)).await?;
        for mut session in sessions {
            if self.sessions.contains_key(&session.id) {
                continue;
            }
            // Terminal history remains queryable from storage. Stale child
            // rows were repaired in one bulk pass above.
            if !session.status.is_active() {
                continue;
            }
            session.status = SessionStatus::Interrupted;
            session.last_error = Some(ApiError::new(
                "daemon_restarted",
                "the daemon restarted before this session completed; the transcript is read-only",
            ));
            session.updated_at = now();
            if !session.metadata.is_object() {
                session.metadata = json!({});
            }
            session.metadata["restored"] = json!(true);

            let permissions: PermissionMap = Arc::new(Mutex::new(HashMap::new()));
            let questions: QuestionMap = Arc::new(Mutex::new(HashMap::new()));
            let emitter = AgentEventEmitter::new(
                session.id.clone(),
                session.provider.clone(),
                self.events.clone(),
                self.dispatch.clone(),
                Arc::clone(&permissions),
                Arc::clone(&questions),
            );
            let handle = Arc::new(SessionHandle::new(
                session.clone(),
                emitter.clone(),
                Arc::clone(&permissions),
                Arc::clone(&questions),
            ));

            let mut interrupted_turns = Vec::new();
            if let Ok(mut turns) = database.turns.list(&session.id).await {
                for turn in &mut turns {
                    if !turn.status.is_terminal() {
                        turn.status = TurnStatus::Interrupted;
                        turn.completed_at = Some(now());
                        interrupted_turns.push(turn.clone());
                    }
                }
                *handle.turns.lock() = turns;
            }
            if let Ok(messages) = database.messages.list(&session.id).await {
                *handle.messages.lock() = messages;
            }
            let mut cancelled_tools = Vec::new();
            if let Ok(mut tools) = database.tool_calls.list(&session.id).await {
                for tool in &mut tools {
                    if matches!(
                        tool.status,
                        ToolCallStatus::Pending | ToolCallStatus::Running
                    ) {
                        tool.status = ToolCallStatus::Cancelled;
                        tool.completed_at = Some(now());
                        tool.error = Some("daemon restarted".to_string());
                        cancelled_tools.push(tool.clone());
                    }
                }
                *handle.tools.lock() = tools;
            }
            handle.trim_history();
            let mut cancelled_permissions = Vec::new();
            if let Ok(stored_permissions) = database.permissions.list(&session.id).await {
                let mut table = permissions.lock();
                for mut permission in stored_permissions {
                    if permission.status == PermissionStatus::Pending {
                        permission.status = PermissionStatus::Cancelled;
                        permission.resolved_at = Some(now());
                        cancelled_permissions.push(permission.clone());
                    }
                    table.insert(
                        permission.id.clone(),
                        PendingPermission {
                            request: permission,
                            sender: None,
                        },
                    );
                }
            }
            let mut cancelled_questions = Vec::new();
            if let Ok(stored_questions) = database.questions.list(&session.id).await {
                let mut table = questions.lock();
                for mut question in stored_questions {
                    if question.status == QuestionStatus::Pending {
                        question.status = QuestionStatus::Cancelled;
                        question.resolved_at = Some(now());
                        cancelled_questions.push(question.clone());
                    }
                    table.insert(
                        question.id.clone(),
                        PendingQuestion {
                            request: question,
                            sender: None,
                        },
                    );
                }
            }
            self.sessions
                .insert(session.id.clone(), Arc::clone(&handle));
            self.persist_session(&session).await;

            emitter.emit(
                AgentEventType::AgentStatusChanged,
                serde_json::to_value(StatusChangedPayload {
                    status: SessionStatus::Interrupted,
                    reason: Some("daemon restarted".to_string()),
                })
                .unwrap_or_default(),
            );
            for turn in interrupted_turns {
                emitter.emit_with(
                    AgentEventType::TurnInterrupted,
                    serde_json::to_value(&turn).unwrap_or_default(),
                    Some(turn.id.clone()),
                    None,
                    None,
                );
            }
            for tool in cancelled_tools {
                emitter.emit_with(
                    AgentEventType::ToolFailed,
                    json!({ "tool": tool.clone(), "error": tool.error.clone() }),
                    tool.turn_id.clone(),
                    None,
                    Some(tool.id.clone()),
                );
            }
            for permission in cancelled_permissions {
                emitter.emit_with(
                    AgentEventType::PermissionRejected,
                    serde_json::to_value(PermissionEventPayload {
                        permission: permission.clone(),
                    })
                    .unwrap_or_default(),
                    permission.turn_id.clone(),
                    None,
                    permission.tool_call_id.clone(),
                );
            }
            for question in cancelled_questions {
                emitter.emit_with(
                    AgentEventType::QuestionRejected,
                    serde_json::to_value(QuestionEventPayload {
                        question: question.clone(),
                    })
                    .unwrap_or_default(),
                    question.turn_id.clone(),
                    None,
                    question.tool_call_id.clone(),
                );
            }
        }
        Ok(())
    }

    /// Descriptors of all registered providers.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.registry.infos()
    }

    /// Subscribe to the unified event bus.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    /// Event stream published after the runtime has applied and recorded each
    /// event. Durable events carry the daemon outbox cursor used for
    /// acknowledgement; ephemeral deltas do not.
    pub fn subscribe_processed(&self) -> broadcast::Receiver<ProcessedAgentEvent> {
        self.processed.subscribe()
    }

    /// Unified event stream for this runtime.
    ///
    /// The stream reconnects transparently after broadcast lag and ends when
    /// the runtime's event sender is dropped.
    pub fn event_stream(&self) -> crate::AgentEventStream {
        let mut receiver = self.subscribe();
        Box::pin(async_stream::stream! {
            loop {
                match receiver.recv().await {
                    Ok(event) => yield Ok::<_, RuntimeError>(event),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "agent event stream lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }

    /// Start a new agent session.
    pub async fn start_session(
        &self,
        machine_id: &str,
        provider_id: &str,
        config: AgentStartConfig,
    ) -> Result<AgentSession> {
        let provider = self
            .registry
            .get(provider_id)
            .ok_or_else(|| RuntimeError::ProviderNotFound(provider_id.to_string()))?;

        let mut session = AgentSession::new(
            machine_id,
            provider.provider_id(),
            provider.agent_type(),
            config.cwd.clone(),
        );
        session.workspace_id = config.workspace_id.clone();
        session.title = config.title.clone();
        session.status = SessionStatus::Creating;

        let permissions: PermissionMap = Arc::new(Mutex::new(HashMap::new()));
        let questions: QuestionMap = Arc::new(Mutex::new(HashMap::new()));
        let emitter = AgentEventEmitter::new(
            session.id.clone(),
            provider.provider_id(),
            self.events.clone(),
            self.dispatch.clone(),
            Arc::clone(&permissions),
            Arc::clone(&questions),
        );
        let handle = Arc::new(SessionHandle::new(
            session.clone(),
            emitter.clone(),
            permissions,
            questions,
        ));
        self.sessions
            .insert(session.id.clone(), Arc::clone(&handle));

        emitter.emit(
            AgentEventType::SessionCreated,
            json!({"session": session.clone()}),
        );

        // Move to `Starting` without a status event so that the first status
        // event observed by clients is the provider's `Running`.
        let starting = {
            let mut guard = handle.session.write().await;
            guard.status = SessionStatus::Starting;
            guard.updated_at = now();
            guard.clone()
        };
        self.persist_session(&starting).await;

        let ctx = ProviderContext {
            machine_id: machine_id.to_string(),
            session: starting,
            emitter: emitter.clone(),
        };

        match provider.start(ctx, config).await {
            Ok(started) => {
                {
                    let mut guard = handle.session.write().await;
                    if !started.metadata.is_null() {
                        guard.metadata = started.metadata.clone();
                    }
                    if started.title.is_some() {
                        guard.title = started.title.clone();
                    }
                    guard.agent_type = started.agent_type.clone();
                    guard.updated_at = now();
                }
                let _ = self
                    .transition(&handle, SessionStatus::Running, None, false)
                    .await;
                let final_session = handle.session.read().await.clone();
                self.persist_session(&final_session).await;
                Ok(final_session)
            }
            Err(error) => {
                let api = error.to_api_error();
                {
                    let mut guard = handle.session.write().await;
                    guard.last_error = Some(api.clone());
                }
                emitter.emit(
                    AgentEventType::SessionError,
                    serde_json::to_value(SessionErrorPayload { error: api.clone() })
                        .unwrap_or_default(),
                );
                // A failed create must not leave a ghost session in the
                // runtime, the admin database, or the browser list.
                self.sessions.remove(&session.id);
                let database = self.storage.read().clone();
                if let Some(database) = database {
                    if let Err(cleanup) = database.sessions.delete(&session.id).await {
                        tracing::warn!(session = %session.id, error = %cleanup, "failed to remove session after start failure");
                    }
                }
                emitter.emit(
                    AgentEventType::SessionDeleted,
                    json!({"reason": "provider start failed", "error": api}),
                );
                Err(error)
            }
        }
    }

    /// Send a user message to a session.
    ///
    /// The session must be `Running` or `WaitingForInput`. Attachments are
    /// rejected with [`RuntimeError::Unsupported`] when the provider declares
    /// `capabilities.images == false`; providers that accept images receive
    /// them appended to the text as labeled base64 sections (no adapter in this
    /// crate currently accepts images).
    pub async fn send_message(&self, session_id: &str, req: SendMessageRequest) -> Result<()> {
        let handle = self.session_handle(session_id)?;
        if restored_session(&handle).await {
            return Err(RuntimeError::Unsupported(
                "this session was restored after a daemon restart and is read-only; start a new session"
                    .to_string(),
            ));
        }
        let provider = self.provider_for(&handle).await?;

        let status = handle.session.read().await.status;
        if !matches!(
            status,
            SessionStatus::Running | SessionStatus::WaitingForInput
        ) {
            return Err(RuntimeError::InvalidState(format!(
                "cannot send a message while session is {status}"
            )));
        }
        if !handle.in_flight.lock().is_empty() {
            return Err(RuntimeError::InvalidState(
                "a turn is already in flight for this session".to_string(),
            ));
        }

        let content = build_message_content(&req, &provider)?;

        if status == SessionStatus::WaitingForInput {
            let _ = self
                .transition(&handle, SessionStatus::Running, None, false)
                .await;
        }

        let mut turn = AgentTurn::new(session_id);
        turn.status = TurnStatus::Running;
        turn.started_at = now();
        handle.begin_turn(turn.clone());

        let user_message = AgentMessage::new(
            session_id,
            Some(turn.id.clone()),
            MessageRole::User,
            content.clone(),
        );
        handle.record_message(user_message.clone());

        handle.emitter.emit_with(
            AgentEventType::TurnStarted,
            serde_json::to_value(&turn).unwrap_or_default(),
            Some(turn.id.clone()),
            None,
            None,
        );
        handle.emitter.emit_with(
            AgentEventType::MessageCompleted,
            serde_json::to_value(MessageCompletedPayload {
                message: user_message.clone(),
            })
            .unwrap_or_default(),
            Some(turn.id.clone()),
            Some(user_message.id.clone()),
            None,
        );

        if let Err(error) = provider.send_message(session_id, content).await {
            let api = error.to_api_error();
            if let Some(updated) = handle.finish_turn(
                Some(turn.id.as_str()),
                TurnStatus::Failed,
                Some(api.clone()),
            ) {
                handle.emitter.emit_with(
                    AgentEventType::TurnFailed,
                    json!({"error": &api, "turn": &updated}),
                    Some(turn.id.clone()),
                    None,
                    None,
                );
            }
            handle.emitter.emit(
                AgentEventType::SessionError,
                serde_json::to_value(SessionErrorPayload { error: api }).unwrap_or_default(),
            );
            let _ = self
                .transition(
                    &handle,
                    SessionStatus::Failed,
                    Some("send_message failed"),
                    false,
                )
                .await;
            return Err(error);
        }

        Ok(())
    }

    /// Interrupt the active turn of a session.
    pub async fn interrupt(&self, session_id: &str) -> Result<()> {
        let handle = self.session_handle(session_id)?;
        if restored_session(&handle).await {
            return Ok(());
        }
        let status = handle.status().await;
        if status.is_terminal() {
            return Ok(());
        }
        let provider = self.provider_for(&handle).await?;
        provider.interrupt(session_id).await?;
        cancel_pending_permissions(&handle, PermissionStatus::Cancelled, &self.storage).await;
        cancel_pending_questions(&handle, QuestionStatus::Cancelled, &self.storage).await;

        let _ = self
            .transition(
                &handle,
                SessionStatus::Interrupted,
                Some("interrupted"),
                false,
            )
            .await;

        let active: Vec<String> = handle
            .in_flight
            .lock()
            .iter()
            .map(|in_flight| in_flight.turn_id.clone())
            .collect();
        for turn_id in active {
            if let Some(updated) =
                handle.finish_turn(Some(turn_id.as_str()), TurnStatus::Interrupted, None)
            {
                handle.emitter.emit_with(
                    AgentEventType::TurnInterrupted,
                    serde_json::to_value(&updated).unwrap_or_default(),
                    Some(turn_id),
                    None,
                    None,
                );
            }
        }
        Ok(())
    }

    /// Resume a session.
    pub async fn resume(&self, session_id: &str) -> Result<()> {
        let handle = self.session_handle(session_id)?;
        if restored_session(&handle).await {
            return Err(RuntimeError::Unsupported(
                "this session was restored after a daemon restart and cannot be resumed; start a new session"
                    .to_string(),
            ));
        }
        let status = handle.status().await;
        if status != SessionStatus::Interrupted {
            return Err(RuntimeError::InvalidState(format!(
                "cannot resume a session that is {status}"
            )));
        }
        let provider = self.provider_for(&handle).await?;
        if !provider.capabilities().resume {
            return Err(RuntimeError::Unsupported(format!(
                "provider `{}` does not support resume",
                provider.provider_id()
            )));
        }
        provider.resume(session_id).await?;
        let _ = self
            .transition(&handle, SessionStatus::Running, Some("resumed"), false)
            .await;
        Ok(())
    }

    /// Shut down a session. The session record remains queryable in its
    /// terminal state.
    pub async fn shutdown_session(&self, session_id: &str) -> Result<()> {
        let handle = self.session_handle(session_id)?;
        if !restored_session(&handle).await {
            let provider = self.provider_for(&handle).await?;
            if let Err(error) = provider.shutdown(session_id).await {
                if !matches!(error, RuntimeError::SessionNotFound(_)) {
                    return Err(error);
                }
            }
        }
        handle.cancel.cancel();
        cancel_pending_permissions(&handle, PermissionStatus::Cancelled, &self.storage).await;
        cancel_pending_questions(&handle, QuestionStatus::Cancelled, &self.storage).await;
        let active: Vec<String> = handle
            .in_flight
            .lock()
            .iter()
            .map(|in_flight| in_flight.turn_id.clone())
            .collect();
        for turn_id in active {
            if let Some(updated) =
                handle.finish_turn(Some(turn_id.as_str()), TurnStatus::Interrupted, None)
            {
                handle.emitter.emit_with(
                    AgentEventType::TurnInterrupted,
                    serde_json::to_value(&updated).unwrap_or_default(),
                    Some(turn_id),
                    None,
                    None,
                );
            }
        }
        let _ = self
            .transition(&handle, SessionStatus::Terminated, Some("shutdown"), true)
            .await;
        Ok(())
    }

    /// Permanently delete a session and its history.
    ///
    /// Unlike [`Self::shutdown_session`], this removes the runtime handle and
    /// all persisted turns/messages/permissions. The provider process is shut
    /// down first when it is still live.
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let database = self.storage.read().clone();
        let handle = self.sessions.remove(session_id).map(|(_, handle)| handle);
        let provider = match &handle {
            Some(handle) => Some(handle.session.read().await.provider.clone()),
            None => match &database {
                Some(database) => database
                    .sessions
                    .get(session_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|session| session.provider),
                None => None,
            },
        };
        if let Some(handle) = handle {
            if !restored_session(&handle).await {
                let provider_id = handle.session.read().await.provider.clone();
                if let Some(provider) = self.registry.get(&provider_id) {
                    if let Err(error) = provider.shutdown(session_id).await {
                        if !matches!(error, RuntimeError::SessionNotFound(_)) {
                            tracing::warn!(session_id, error = %error, "provider shutdown failed during delete");
                        }
                    }
                }
            }
            handle.cancel.cancel();
            cancel_pending_permissions(&handle, PermissionStatus::Cancelled, &self.storage).await;
            cancel_pending_questions(&handle, QuestionStatus::Cancelled, &self.storage).await;
        }

        // Projection removal, `SessionDeleted` outbox append and discarding
        // older pending events for this session commit atomically. The admin
        // can no longer prove ownership after its row is gone, so leaving
        // those older rows pending would stall the contiguous cursor forever.
        let mut event = AgentEvent::new(
            session_id,
            1,
            AgentEventType::SessionDeleted,
            provider.unwrap_or_else(|| "unknown".to_string()),
        );
        event.payload = json!({"reason": "deleted"});
        if let Some(database) = &database {
            database.record_session_deleted(&event).await?;
        }
        let _ = self.dispatch.send(event.clone());
        let _ = self.events.send(event);
        Ok(())
    }

    /// Shut down every session. Errors are logged and ignored.
    pub async fn shutdown_all(&self) {
        let ids: Vec<String> = self
            .sessions
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for id in ids {
            if let Err(error) = self.shutdown_session(&id).await {
                tracing::warn!(session_id = %id, error = %error, "failed to shut down session");
            }
        }
    }

    /// Resolve a permission request by id.
    pub async fn respond_permission(&self, decision: PermissionDecision) -> Result<()> {
        let handle = self
            .sessions
            .iter()
            .find(|entry| {
                entry
                    .value()
                    .permissions
                    .lock()
                    .contains_key(&decision.permission_id)
            })
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| RuntimeError::PermissionNotFound(decision.permission_id.clone()))?;

        let (permission, sender) = {
            let mut permissions = handle.permissions.lock();
            let entry = permissions
                .get_mut(&decision.permission_id)
                .ok_or_else(|| RuntimeError::PermissionNotFound(decision.permission_id.clone()))?;
            if decision
                .session_id
                .as_ref()
                .is_some_and(|session_id| session_id != &entry.request.session_id)
            {
                return Err(RuntimeError::PermissionNotFound(
                    decision.permission_id.clone(),
                ));
            }
            if let Some(option_id) = &decision.option_id {
                if !entry
                    .request
                    .options
                    .iter()
                    .any(|option| &option.option_id == option_id)
                {
                    return Err(RuntimeError::InvalidRequest(format!(
                        "permission option `{option_id}` was not offered"
                    )));
                }
            }
            if entry.request.status != PermissionStatus::Pending {
                return Err(RuntimeError::Conflict(format!(
                    "permission {} is already {:?}",
                    decision.permission_id, entry.request.status
                )));
            }
            entry.request.status = if decision.approve {
                PermissionStatus::Approved
            } else {
                PermissionStatus::Rejected
            };
            entry.request.resolved_at = Some(now());
            (entry.request.clone(), entry.sender.take())
        };

        let event_type = if decision.approve {
            AgentEventType::PermissionApproved
        } else {
            AgentEventType::PermissionRejected
        };
        handle.emitter.emit_with(
            event_type,
            serde_json::to_value(PermissionEventPayload {
                permission: permission.clone(),
            })
            .unwrap_or_default(),
            permission.turn_id.clone(),
            None,
            permission.tool_call_id.clone(),
        );

        let hub_resolved = sender.is_some();
        if let Some(sender) = sender {
            if sender.send(decision.clone()).is_err() {
                let mut failed = permission.clone();
                failed.status = PermissionStatus::DeliveryFailed;
                failed.resolved_at = Some(now());
                {
                    let mut permissions = handle.permissions.lock();
                    if let Some(entry) = permissions.get_mut(&failed.id) {
                        entry.request = failed.clone();
                    }
                }
                handle.emitter.emit_with(
                    AgentEventType::PermissionRejected,
                    serde_json::to_value(PermissionEventPayload { permission: failed })
                        .unwrap_or_default(),
                    permission.turn_id.clone(),
                    None,
                    permission.tool_call_id.clone(),
                );
                return Err(RuntimeError::Provider {
                    code: fleet_core_common::error_codes::CONFLICT.to_string(),
                    message: "the provider no longer accepts this permission decision".to_string(),
                });
            }
        }

        if !hub_resolved {
            // No runtime-owned sender: the provider must deliver the decision
            // over its native approval channel.
            let provider = self.provider_for(&handle).await?;
            provider
                .respond_permission(&permission.session_id, decision.clone())
                .await?;
        }

        if handle.status().await == SessionStatus::WaitingForPermission {
            let _ = self
                .transition(
                    &handle,
                    SessionStatus::Running,
                    Some("permission resolved"),
                    false,
                )
                .await;
        }
        Ok(())
    }

    /// Resolve a user question by id.
    pub async fn respond_question(&self, decision: QuestionDecision) -> Result<()> {
        let handle = self
            .sessions
            .iter()
            .find(|entry| {
                entry
                    .value()
                    .questions
                    .lock()
                    .contains_key(&decision.question_id)
            })
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| RuntimeError::QuestionNotFound(decision.question_id.clone()))?;

        let (question, sender) = {
            let mut questions = handle.questions.lock();
            let entry = questions
                .get_mut(&decision.question_id)
                .ok_or_else(|| RuntimeError::QuestionNotFound(decision.question_id.clone()))?;
            if decision
                .session_id
                .as_ref()
                .is_some_and(|session_id| session_id != &entry.request.session_id)
            {
                return Err(RuntimeError::QuestionNotFound(decision.question_id.clone()));
            }
            if entry.request.status.is_terminal() {
                return Err(RuntimeError::Conflict(format!(
                    "question {} is already {:?}",
                    decision.question_id, entry.request.status
                )));
            }
            validate_question_decision(&entry.request, &decision)?;
            entry.request.status = if decision.reject {
                QuestionStatus::Rejected
            } else {
                QuestionStatus::Answered
            };
            entry.request.resolved_at = Some(now());
            entry.request.answers = decision.answers.clone();
            (entry.request.clone(), entry.sender.take())
        };

        let event_type = if decision.reject {
            AgentEventType::QuestionRejected
        } else {
            AgentEventType::QuestionAnswered
        };
        handle.emitter.emit_with(
            event_type,
            serde_json::to_value(QuestionEventPayload {
                question: question.clone(),
            })
            .unwrap_or_default(),
            question.turn_id.clone(),
            None,
            question.tool_call_id.clone(),
        );

        if let Some(sender) = sender {
            if sender.send(decision.clone()).is_err() {
                let mut failed = question.clone();
                failed.status = QuestionStatus::DeliveryFailed;
                failed.resolved_at = Some(now());
                {
                    let mut questions = handle.questions.lock();
                    if let Some(entry) = questions.get_mut(&failed.id) {
                        entry.request = failed.clone();
                    }
                }
                handle.emitter.emit_with(
                    AgentEventType::QuestionRejected,
                    serde_json::to_value(QuestionEventPayload {
                        question: failed.clone(),
                    })
                    .unwrap_or_default(),
                    failed.turn_id.clone(),
                    None,
                    failed.tool_call_id.clone(),
                );
                let database = self.storage.read().clone();
                if let Some(database) = database {
                    if let Err(error) = database.questions.upsert(&failed).await {
                        tracing::warn!(question = %failed.id, error = %error, "failed to persist delivery failure");
                    }
                }
                return Err(RuntimeError::Provider {
                    code: fleet_core_common::error_codes::CONFLICT.to_string(),
                    message: "the provider no longer accepts this question decision".to_string(),
                });
            }
        } else {
            let provider = self.provider_for(&handle).await?;
            provider
                .respond_question(&question.session_id, decision.clone())
                .await?;
        }

        if handle.status().await == SessionStatus::WaitingForQuestion
            && !has_pending_question(&handle)
        {
            let _ = self
                .transition(
                    &handle,
                    SessionStatus::Running,
                    Some("question resolved"),
                    false,
                )
                .await;
        }
        Ok(())
    }

    /// All known sessions, oldest first.
    ///
    /// Sessions without a live handle (terminal or interrupted sessions from an
    /// earlier daemon run) are read from storage so no session becomes
    /// unreachable after a restart.
    pub async fn list_sessions(&self) -> Vec<AgentSession> {
        let mut by_id: HashMap<String, AgentSession> = HashMap::new();
        let database = self.storage.read().clone();
        if let Some(database) = database {
            if let Ok(stored) = database.sessions.list(None).await {
                for session in stored {
                    by_id.insert(session.id.clone(), session);
                }
            }
        }
        for entry in self.sessions.iter() {
            let session = entry.value().session.read().await.clone();
            by_id.insert(session.id.clone(), session);
        }
        let mut sessions: Vec<AgentSession> = by_id.into_values().collect();
        sessions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        sessions
    }

    /// Look up one session.
    pub async fn get_session(&self, id: &str) -> Option<AgentSession> {
        if let Some(handle) = self.sessions.get(id).map(|entry| Arc::clone(entry.value())) {
            return Some(handle.session.read().await.clone());
        }
        let database = self.storage.read().clone()?;
        match database.sessions.get(id).await {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(session_id = %id, error = %error, "failed to read session");
                None
            }
        }
    }

    /// Full session detail: turns, messages and permission requests.
    pub async fn session_detail(&self, id: &str) -> Option<SessionDetail> {
        let database = self.storage.read().clone();
        let handle = self.sessions.get(id).map(|entry| Arc::clone(entry.value()));

        let session = match &handle {
            Some(handle) => handle.session.read().await.clone(),
            None => match &database {
                Some(database) => match database.sessions.get(id).await {
                    Ok(session) => session?,
                    Err(error) => {
                        tracing::warn!(session_id = %id, error = %error, "failed to read session");
                        return None;
                    }
                },
                None => return None,
            },
        };

        let (mut turns, mut messages, mut tools, mut permissions, mut questions) = match &handle {
            Some(handle) => {
                let permissions = handle
                    .permissions
                    .lock()
                    .values()
                    .map(|entry| entry.request.clone())
                    .collect();
                let questions = handle
                    .questions
                    .lock()
                    .values()
                    .map(|entry| entry.request.clone())
                    .collect();
                (
                    handle.turns.lock().clone(),
                    handle.messages.lock().clone(),
                    handle.tools.lock().clone(),
                    permissions,
                    questions,
                )
            }
            None => (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        };

        if let Some(database) = database {
            if let Ok(stored) = database.turns.list(id).await {
                turns = merge_by_id(turns, stored, |turn| turn.id.clone());
            }
            if let Ok(stored) = database.messages.list(id).await {
                messages = merge_by_id(messages, stored, |message| message.id.clone());
            }
            if let Ok(stored) = database.tool_calls.list(id).await {
                tools = merge_tools(tools, stored);
            }
            if let Ok(stored) = database.permissions.list(id).await {
                permissions = merge_by_id(permissions, stored, |permission| permission.id.clone());
            }
            if let Ok(stored) = database.questions.list(id).await {
                questions = merge_by_id(questions, stored, |question| question.id.clone());
            }
        }
        turns.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        messages.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        tools.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        permissions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        questions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Some(SessionDetail {
            session,
            turns,
            messages,
            tool_calls: tools,
            permissions,
            questions,
        })
    }

    fn session_handle(&self, id: &str) -> Result<Arc<SessionHandle>> {
        self.sessions
            .get(id)
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| RuntimeError::SessionNotFound(id.to_string()))
    }

    async fn provider_for(&self, handle: &SessionHandle) -> Result<Arc<dyn AgentProvider>> {
        let provider_id = handle.session.read().await.provider.clone();
        self.registry
            .get(&provider_id)
            .ok_or(RuntimeError::ProviderNotFound(provider_id))
    }

    async fn transition(
        &self,
        handle: &SessionHandle,
        next: SessionStatus,
        reason: Option<&str>,
        force: bool,
    ) -> Result<bool> {
        let current = handle.status().await;
        if current == next {
            return Ok(false);
        }

        let changed = {
            let mut guard = handle.session.write().await;
            if guard.status == next {
                false
            } else if guard.status.can_transition_to(next) || force {
                guard.status = next;
                guard.updated_at = now();
                true
            } else {
                tracing::warn!(
                    session_id = %guard.id,
                    from = %guard.status,
                    to = %next,
                    "rejected illegal session status transition"
                );
                false
            }
        };
        if !changed {
            return Ok(false);
        }

        handle.emitter.emit(
            AgentEventType::AgentStatusChanged,
            serde_json::to_value(StatusChangedPayload {
                status: next,
                reason: reason.map(str::to_string),
            })
            .unwrap_or_default(),
        );
        Ok(true)
    }

    async fn persist_session(&self, session: &AgentSession) {
        persist_session(&self.storage, session).await;
    }
}

async fn restored_session(handle: &Arc<SessionHandle>) -> bool {
    handle
        .session
        .read()
        .await
        .metadata
        .get("restored")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn merge_tools(
    mut existing: Vec<AgentToolCall>,
    incoming: Vec<AgentToolCall>,
) -> Vec<AgentToolCall> {
    for item in incoming {
        if !existing.iter().any(|candidate| candidate.id == item.id) {
            existing.push(item);
        }
    }
    existing
}

/// Merge persisted rows into the in-memory transcript without overwriting
/// rows the runtime has already updated. Memory is the freshest view; storage
/// may lag by one dispatcher transaction. Persisted rows still fill history
/// that was trimmed from memory.
fn merge_by_id<T, F>(mut existing: Vec<T>, incoming: Vec<T>, id: F) -> Vec<T>
where
    F: Fn(&T) -> String,
{
    for item in incoming {
        let item_id = id(&item);
        if !existing.iter().any(|candidate| id(candidate) == item_id) {
            existing.push(item);
        }
    }
    existing
}

fn build_message_content(
    request: &SendMessageRequest,
    provider: &Arc<dyn AgentProvider>,
) -> Result<String> {
    if request.attachments.is_empty() {
        return Ok(request.content.clone());
    }
    if !provider.capabilities().images {
        return Err(RuntimeError::Unsupported(format!(
            "provider `{}` does not accept message attachments",
            provider.provider_id()
        )));
    }
    let mut content = request.content.clone();
    for attachment in &request.attachments {
        content.push_str("\n\n[attachment: ");
        content.push_str(&attachment.name);
        if let Some(mime) = &attachment.mime_type {
            content.push_str(" (");
            content.push_str(mime);
            content.push(')');
        }
        content.push_str("]\n");
        content.push_str(&attachment.data_base64);
    }
    Ok(content)
}

async fn dispatch_events(
    sessions: Arc<DashMap<String, Arc<SessionHandle>>>,
    storage: StorageHandle,
    processed: broadcast::Sender<ProcessedAgentEvent>,
    mut receiver: mpsc::UnboundedReceiver<AgentEvent>,
) {
    while let Some(mut event) = receiver.recv().await {
        let handle = sessions
            .get(&event.session_id)
            .map(|entry| Arc::clone(entry.value()));

        // Events for an already-removed session still matter: a deletion must
        // reach the outbox (and therefore the admin) even when no handle is
        // left to update.
        if let Some(handle) = handle {
            match event.event_type {
                AgentEventType::AgentRawEvent => {
                    let provider_session_id = event
                        .payload
                        .get("provider_session_id")
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| {
                            event
                                .raw_event
                                .as_ref()
                                .and_then(|raw| raw.get("provider_session_id"))
                                .and_then(serde_json::Value::as_str)
                        })
                        .map(str::to_string);
                    if let Some(native_id) = provider_session_id {
                        let updated = {
                            let mut guard = handle.session.write().await;
                            if !guard.metadata.is_object() {
                                guard.metadata = json!({});
                            }
                            guard.metadata["provider_session_id"] = json!(native_id);
                            guard.updated_at = now();
                            guard.clone()
                        };
                        persist_session(&storage, &updated).await;
                    }
                }
                AgentEventType::AgentStatusChanged => {
                    if let Ok(payload) =
                        serde_json::from_value::<StatusChangedPayload>(event.payload.clone())
                    {
                        apply_status(&handle, payload.status).await;
                    }
                }
                AgentEventType::SessionError => {
                    if let Ok(payload) =
                        serde_json::from_value::<SessionErrorPayload>(event.payload.clone())
                    {
                        apply_error(&handle, payload.error).await;
                    }
                }
                AgentEventType::TurnStarted => {
                    if let Ok(turn) = serde_json::from_value::<AgentTurn>(event.payload.clone()) {
                        handle.upsert_turn(turn);
                    }
                }
                AgentEventType::TurnCompleted => {
                    handle.finish_turn(event.turn_id.as_deref(), TurnStatus::Completed, None);
                }
                AgentEventType::TurnInterrupted => {
                    handle.finish_turn(event.turn_id.as_deref(), TurnStatus::Interrupted, None);
                }
                AgentEventType::TurnFailed => {
                    let error = event
                        .payload
                        .get("error")
                        .and_then(|value| serde_json::from_value::<ApiError>(value.clone()).ok());
                    handle.finish_turn(event.turn_id.as_deref(), TurnStatus::Failed, error);
                }
                AgentEventType::MessageCompleted => {
                    let message =
                        serde_json::from_value::<MessageCompletedPayload>(event.payload.clone())
                            .map(|payload| payload.message)
                            .or_else(|_| {
                                serde_json::from_value::<AgentMessage>(event.payload.clone())
                            });
                    if let Ok(message) = message {
                        handle.record_message(message);
                    }
                }
                AgentEventType::ToolStarted | AgentEventType::ToolCompleted => {
                    if let Some(tool) = parse_tool(&event.payload) {
                        let merged = handle.upsert_tool(tool);
                        event.payload = json!({ "tool": merged });
                    }
                }
                AgentEventType::ToolFailed => {
                    if let Some(tool) = parse_tool(&event.payload) {
                        let merged = handle.upsert_tool(tool);
                        event.payload =
                            json!({ "tool": merged.clone(), "error": merged.error.clone() });
                    } else if let Some(status) = parse_tool_status(&event.payload) {
                        handle.finish_tool(
                            &status.tool_call_id,
                            status.status,
                            status.error.clone(),
                        );
                    }
                }
                AgentEventType::PermissionRequested => {
                    if let Some(permission) = parse_permission(&event.payload) {
                        let still_pending = {
                            let mut permissions = handle.permissions.lock();
                            let entry =
                                permissions.entry(permission.id.clone()).or_insert_with(|| {
                                    crate::event::PendingPermission {
                                        request: permission.clone(),
                                        sender: None,
                                    }
                                });
                            entry.request.status == PermissionStatus::Pending
                        };
                        // A decision may already have arrived if the caller reacted
                        // to the event before this dispatcher task ran. In that
                        // case leave the session in its current state.
                        if still_pending {
                            apply_status(&handle, SessionStatus::WaitingForPermission).await;
                        }
                    }
                }
                AgentEventType::PermissionApproved | AgentEventType::PermissionRejected => {
                    if let Some(permission) = parse_permission(&event.payload) {
                        {
                            let mut permissions = handle.permissions.lock();
                            if let Some(entry) = permissions.get_mut(&permission.id) {
                                entry.request.status = permission.status;
                                entry.request.resolved_at = permission.resolved_at;
                            }
                        }
                        if handle.status().await == SessionStatus::WaitingForPermission {
                            apply_status(&handle, SessionStatus::Running).await;
                        }
                    }
                }
                AgentEventType::QuestionRequested => {
                    if let Some(question) = parse_question(&event.payload) {
                        let still_pending = {
                            let mut questions = handle.questions.lock();
                            let entry = questions.entry(question.id.clone()).or_insert_with(|| {
                                PendingQuestion {
                                    request: question.clone(),
                                    sender: None,
                                }
                            });
                            entry.request.status == QuestionStatus::Pending
                        };
                        if still_pending {
                            apply_status(&handle, SessionStatus::WaitingForQuestion).await;
                        }
                    }
                }
                AgentEventType::QuestionAnswered | AgentEventType::QuestionRejected => {
                    if let Some(question) = parse_question(&event.payload) {
                        {
                            let mut questions = handle.questions.lock();
                            if let Some(entry) = questions.get_mut(&question.id) {
                                entry.request.status = question.status;
                                entry.request.resolved_at = question.resolved_at;
                                entry.request.answers = question.answers;
                            }
                        }
                        if handle.status().await == SessionStatus::WaitingForQuestion
                            && !has_pending_question(&handle)
                        {
                            apply_status(&handle, SessionStatus::Running).await;
                        }
                    }
                }
                _ => {}
            }
        }

        // Durable projection + outbox in one SQLite transaction. Ephemeral
        // deltas are ignored by the store. A transient busy/locked error is
        // retried before the event is treated as unrecorded.
        let database = storage.read().clone();
        let cursor = match database {
            Some(database) => {
                let mut attempt = 0;
                loop {
                    match database.record_event(&event).await {
                        Ok(cursor) => break cursor,
                        Err(error) if attempt < 3 => {
                            attempt += 1;
                            tracing::warn!(
                                session_id = %event.session_id,
                                event_id = %event.event_id,
                                error = %error,
                                attempt,
                                "failed to record agent event; retrying"
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(20 * attempt))
                                .await;
                        }
                        Err(error) => {
                            tracing::warn!(
                                session_id = %event.session_id,
                                event_id = %event.event_id,
                                error = %error,
                                "failed to record agent event in the daemon outbox"
                            );
                            break None;
                        }
                    }
                }
            }
            None => None,
        };
        let _ = processed.send(ProcessedAgentEvent { event, cursor });
    }
}

async fn cancel_pending_permissions(
    handle: &Arc<SessionHandle>,
    status: PermissionStatus,
    _storage: &StorageHandle,
) {
    let pending: Vec<AgentPermissionRequest> = {
        let mut permissions = handle.permissions.lock();
        permissions
            .values_mut()
            .filter_map(|entry| {
                if entry.request.status.is_terminal() {
                    return None;
                }
                entry.request.status = status;
                entry.request.resolved_at = Some(now());
                entry.sender.take();
                Some(entry.request.clone())
            })
            .collect()
    };
    for permission in pending {
        let payload = serde_json::to_value(PermissionEventPayload {
            permission: permission.clone(),
        })
        .unwrap_or_default();
        handle.emitter.emit_with(
            AgentEventType::PermissionRejected,
            payload,
            permission.turn_id.clone(),
            None,
            permission.tool_call_id.clone(),
        );
    }
}

async fn cancel_pending_questions(
    handle: &Arc<SessionHandle>,
    status: QuestionStatus,
    storage: &StorageHandle,
) {
    let pending: Vec<AgentQuestionRequest> = {
        let mut questions = handle.questions.lock();
        questions
            .values_mut()
            .filter_map(|entry| {
                if entry.request.status.is_terminal() {
                    return None;
                }
                entry.request.status = status;
                entry.request.resolved_at = Some(now());
                entry.sender.take();
                Some(entry.request.clone())
            })
            .collect()
    };
    let database = storage.read().clone();
    for question in pending {
        handle.emitter.emit_with(
            AgentEventType::QuestionRejected,
            serde_json::to_value(QuestionEventPayload {
                question: question.clone(),
            })
            .unwrap_or_default(),
            question.turn_id.clone(),
            None,
            question.tool_call_id.clone(),
        );
        if let Some(database) = &database {
            if let Err(error) = database.questions.upsert(&question).await {
                tracing::warn!(question = %question.id, error = %error, "failed to persist cancelled question");
            }
        }
    }
}

fn parse_tool(payload: &serde_json::Value) -> Option<AgentToolCall> {
    payload
        .get("tool")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or_else(|| serde_json::from_value(payload.clone()).ok())
}

fn parse_tool_status(payload: &serde_json::Value) -> Option<ToolStatusPayload> {
    serde_json::from_value(payload.clone()).ok()
}

fn parse_permission(payload: &serde_json::Value) -> Option<AgentPermissionRequest> {
    payload
        .get("permission")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or_else(|| serde_json::from_value(payload.clone()).ok())
}

fn has_pending_question(handle: &Arc<SessionHandle>) -> bool {
    handle
        .questions
        .lock()
        .values()
        .any(|entry| entry.request.status == QuestionStatus::Pending)
}

fn parse_question(payload: &serde_json::Value) -> Option<AgentQuestionRequest> {
    payload
        .get("question")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or_else(|| serde_json::from_value(payload.clone()).ok())
}

const MAX_QUESTION_CUSTOM_BYTES: usize = 8 * 1024;

fn validate_question_decision(
    question: &AgentQuestionRequest,
    decision: &QuestionDecision,
) -> Result<()> {
    if decision.reject {
        return Ok(());
    }
    if decision.answers.len() != question.questions.len() {
        return Err(RuntimeError::InvalidRequest(
            "every question must have exactly one answer".to_string(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for answer in &decision.answers {
        if !seen.insert(answer.id.as_str()) {
            return Err(RuntimeError::InvalidRequest(format!(
                "duplicate answer for question `{}`",
                answer.id
            )));
        }
        let Some(item) = question.questions.iter().find(|item| item.id == answer.id) else {
            return Err(RuntimeError::InvalidRequest(format!(
                "unknown question id `{}`",
                answer.id
            )));
        };
        if !item.multi_select && answer.selected.len() > 1 {
            return Err(RuntimeError::InvalidRequest(format!(
                "question `{}` accepts a single selection",
                item.id
            )));
        }
        for selected in &answer.selected {
            if !item.options.iter().any(|option| &option.label == selected) {
                return Err(RuntimeError::InvalidRequest(format!(
                    "option `{selected}` was not offered for question `{}`",
                    item.id
                )));
            }
        }
        if let Some(custom) = &answer.custom {
            if !item.allow_custom {
                return Err(RuntimeError::InvalidRequest(format!(
                    "question `{}` does not accept a custom answer",
                    item.id
                )));
            }
            if custom.len() > MAX_QUESTION_CUSTOM_BYTES {
                return Err(RuntimeError::InvalidRequest(format!(
                    "custom answer for question `{}` is too long",
                    item.id
                )));
            }
        }
    }
    Ok(())
}

async fn apply_status(handle: &Arc<SessionHandle>, next: SessionStatus) {
    let mut guard = handle.session.write().await;
    if guard.status == next {
        return;
    }
    if guard.status.can_transition_to(next) {
        guard.status = next;
        guard.updated_at = now();
    } else {
        tracing::debug!(
            session_id = %guard.id,
            from = %guard.status,
            to = %next,
            "ignored status event that would be an illegal transition"
        );
    }
}

async fn apply_error(handle: &Arc<SessionHandle>, error: ApiError) {
    let mut guard = handle.session.write().await;
    guard.last_error = Some(error);
    if guard.status != SessionStatus::Failed
        && guard.status.can_transition_to(SessionStatus::Failed)
    {
        guard.status = SessionStatus::Failed;
    }
    guard.updated_at = now();
}

async fn persist_session(storage: &StorageHandle, session: &AgentSession) {
    let database = storage.read().clone();
    if let Some(database) = database {
        if let Err(error) = database.sessions.upsert(session).await {
            tracing::warn!(session_id = %session.id, error = %error, "failed to persist session");
        }
    }
}

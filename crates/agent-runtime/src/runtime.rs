//! Unified runtime: session registry, lifecycle state machine, event dispatch
//! and provider selection.

use std::collections::HashMap;
use std::sync::Arc;

use agent_fleet_common::{now, ApiError};
use agent_fleet_protocol::agent::{
    AgentEvent, AgentEventType, AgentMessage, AgentPermissionRequest, AgentSession,
    AgentStartConfig, AgentTurn, MessageCompletedPayload, MessageRole, PermissionDecision,
    PermissionEventPayload, PermissionStatus, ProviderInfo, SendMessageRequest,
    SessionErrorPayload, SessionStatus, StatusChangedPayload, TurnStatus,
};
use agent_fleet_protocol::rpc::SessionDetail;
use agent_fleet_storage::Database;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use serde_json::json;
use tokio::sync::broadcast;

use crate::error::{Result, RuntimeError};
use crate::event::{AgentEventEmitter, PermissionMap};
use crate::provider::{AgentProvider, ProviderContext, ProviderRegistry};
use crate::session::SessionHandle;

type StorageHandle = Arc<RwLock<Option<Arc<Database>>>>;

/// Unified agent runtime.
pub struct AgentRuntime {
    registry: ProviderRegistry,
    sessions: Arc<DashMap<String, Arc<SessionHandle>>>,
    events: broadcast::Sender<AgentEvent>,
    storage: StorageHandle,
}

impl AgentRuntime {
    /// Create a runtime from a provider registry.
    ///
    /// `event_capacity` is the broadcast buffer size; values below one are
    /// clamped to one.
    pub fn new(registry: ProviderRegistry, event_capacity: usize) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        let sessions: Arc<DashMap<String, Arc<SessionHandle>>> = Arc::new(DashMap::new());
        let storage: StorageHandle = Arc::new(RwLock::new(None));

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let dispatch_sessions = Arc::clone(&sessions);
            let dispatch_storage = Arc::clone(&storage);
            let receiver = events.subscribe();
            handle.spawn(dispatch_events(
                dispatch_sessions,
                dispatch_storage,
                receiver,
            ));
        } else {
            tracing::warn!(
                "AgentRuntime created outside a Tokio runtime; event state tracking is inactive"
            );
        }

        Self {
            registry,
            sessions,
            events,
            storage,
        }
    }

    /// Attach optional persistence. Sessions and permissions are then upserted
    /// as the runtime observes lifecycle and permission events.
    pub fn with_storage(self, storage: Arc<Database>) -> Self {
        *self.storage.write() = Some(storage);
        self
    }

    /// Descriptors of all registered providers.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.registry.infos()
    }

    /// Subscribe to the unified event bus.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
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
        let emitter = AgentEventEmitter::new(
            session.id.clone(),
            provider.provider_id(),
            self.events.clone(),
            Arc::clone(&permissions),
        );
        let handle = Arc::new(SessionHandle::new(
            session.clone(),
            emitter.clone(),
            permissions,
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
                    serde_json::to_value(SessionErrorPayload { error: api }).unwrap_or_default(),
                );
                let _ = self
                    .transition(
                        &handle,
                        SessionStatus::Failed,
                        Some("provider start failed"),
                        false,
                    )
                    .await;
                let failed = handle.session.read().await.clone();
                self.persist_session(&failed).await;
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
        handle.record_message(user_message);

        handle.emitter.emit_with(
            AgentEventType::TurnStarted,
            serde_json::to_value(&turn).unwrap_or_default(),
            Some(turn.id.clone()),
            None,
            None,
        );

        let current = handle.session.read().await.clone();
        self.persist_session(&current).await;

        if let Err(error) = provider.send_message(session_id, content).await {
            let api = error.to_api_error();
            if let Some(updated) = handle.finish_turn(
                Some(turn.id.as_str()),
                TurnStatus::Failed,
                Some(api.clone()),
            ) {
                handle.emitter.emit_with(
                    AgentEventType::TurnFailed,
                    json!({"error": api.clone(), "turn": updated}),
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
        let status = handle.status().await;
        if status.is_terminal() {
            return Ok(());
        }
        let provider = self.provider_for(&handle).await?;
        provider.interrupt(session_id).await?;
        cancel_pending_permissions(&handle, PermissionStatus::Cancelled, &self.storage).await;

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
        let provider = self.provider_for(&handle).await?;
        if let Err(error) = provider.shutdown(session_id).await {
            if !matches!(error, RuntimeError::SessionNotFound(_)) {
                return Err(error);
            }
        }
        handle.cancel.cancel();
        cancel_pending_permissions(&handle, PermissionStatus::Cancelled, &self.storage).await;
        let active: Vec<String> = handle
            .in_flight
            .lock()
            .iter()
            .map(|in_flight| in_flight.turn_id.clone())
            .collect();
        for turn_id in active {
            let _ = handle.finish_turn(Some(turn_id.as_str()), TurnStatus::Interrupted, None);
        }
        let _ = self
            .transition(&handle, SessionStatus::Terminated, Some("shutdown"), true)
            .await;
        let session = handle.session.read().await.clone();
        self.persist_session(&session).await;
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
            if entry.request.status != PermissionStatus::Pending {
                return Err(RuntimeError::InvalidState(format!(
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

        self.persist_permission(&permission).await;
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
                self.persist_permission(&failed).await;
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
                    code: agent_fleet_common::error_codes::CONFLICT.to_string(),
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

    /// All known sessions, oldest first.
    pub async fn list_sessions(&self) -> Vec<AgentSession> {
        let mut sessions: Vec<AgentSession> = Vec::with_capacity(self.sessions.len());
        for entry in self.sessions.iter() {
            sessions.push(entry.value().session.read().await.clone());
        }
        sessions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        sessions
    }

    /// Look up one session.
    pub async fn get_session(&self, id: &str) -> Option<AgentSession> {
        let handle = self
            .sessions
            .get(id)
            .map(|entry| Arc::clone(entry.value()))?;
        let session = handle.session.read().await.clone();
        Some(session)
    }

    /// Full session detail: turns, messages and permission requests.
    pub async fn session_detail(&self, id: &str) -> Option<SessionDetail> {
        let handle = self
            .sessions
            .get(id)
            .map(|entry| Arc::clone(entry.value()))?;
        let session = handle.session.read().await.clone();
        let turns = handle.turns.lock().clone();
        let messages = handle.messages.lock().clone();
        let mut permissions: Vec<AgentPermissionRequest> = handle
            .permissions
            .lock()
            .values()
            .map(|entry| entry.request.clone())
            .collect();
        permissions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Some(SessionDetail {
            session,
            turns,
            messages,
            permissions,
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
        let session = handle.session.read().await.clone();
        self.persist_session(&session).await;
        Ok(true)
    }

    async fn persist_session(&self, session: &AgentSession) {
        let storage = self.storage.read().clone();
        if let Some(database) = storage {
            if let Err(error) = database.upsert_session(session).await {
                tracing::warn!(session_id = %session.id, error = %error, "failed to persist session");
            }
        }
    }

    async fn persist_permission(&self, permission: &AgentPermissionRequest) {
        let storage = self.storage.read().clone();
        if let Some(database) = storage {
            if let Err(error) = database.upsert_permission(permission).await {
                tracing::warn!(
                    permission_id = %permission.id,
                    error = %error,
                    "failed to persist permission"
                );
            }
        }
    }
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
    mut receiver: broadcast::Receiver<AgentEvent>,
) {
    loop {
        let event = match receiver.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "agent event dispatcher lagged");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };

        let handle = sessions
            .get(&event.session_id)
            .map(|entry| Arc::clone(entry.value()));
        let Some(handle) = handle else {
            continue;
        };

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
                    apply_status(&handle, payload.status, &storage).await;
                }
            }
            AgentEventType::SessionError => {
                if let Ok(payload) =
                    serde_json::from_value::<SessionErrorPayload>(event.payload.clone())
                {
                    apply_error(&handle, payload.error, &storage).await;
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
                        .or_else(|_| serde_json::from_value::<AgentMessage>(event.payload.clone()));
                if let Ok(message) = message {
                    handle.record_message(message);
                }
            }
            AgentEventType::PermissionRequested => {
                if let Some(permission) = parse_permission(&event.payload) {
                    let still_pending = {
                        let mut permissions = handle.permissions.lock();
                        let entry = permissions.entry(permission.id.clone()).or_insert_with(|| {
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
                        apply_status(&handle, SessionStatus::WaitingForPermission, &storage).await;
                    }
                    persist_permission(&storage, &permission).await;
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
                        apply_status(&handle, SessionStatus::Running, &storage).await;
                    }
                    persist_permission(&storage, &permission).await;
                }
            }
            _ => {}
        }
    }
}

async fn cancel_pending_permissions(
    handle: &Arc<SessionHandle>,
    status: PermissionStatus,
    storage: &StorageHandle,
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
        persist_permission(storage, &permission).await;
    }
}

fn parse_permission(payload: &serde_json::Value) -> Option<AgentPermissionRequest> {
    payload
        .get("permission")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or_else(|| serde_json::from_value(payload.clone()).ok())
}

async fn apply_status(handle: &Arc<SessionHandle>, next: SessionStatus, storage: &StorageHandle) {
    let changed = {
        let mut guard = handle.session.write().await;
        if guard.status == next {
            None
        } else if guard.status.can_transition_to(next) {
            guard.status = next;
            guard.updated_at = now();
            Some(guard.clone())
        } else {
            tracing::debug!(
                session_id = %guard.id,
                from = %guard.status,
                to = %next,
                "ignored status event that would be an illegal transition"
            );
            None
        }
    };
    if let Some(session) = changed {
        persist_session(storage, &session).await;
    }
}

async fn apply_error(handle: &Arc<SessionHandle>, error: ApiError, storage: &StorageHandle) {
    let changed = {
        let mut guard = handle.session.write().await;
        guard.last_error = Some(error);
        if guard.status != SessionStatus::Failed
            && guard.status.can_transition_to(SessionStatus::Failed)
        {
            guard.status = SessionStatus::Failed;
            guard.updated_at = now();
            Some(guard.clone())
        } else {
            guard.updated_at = now();
            Some(guard.clone())
        }
    };
    if let Some(session) = changed {
        persist_session(storage, &session).await;
    }
}

async fn persist_session(storage: &StorageHandle, session: &AgentSession) {
    let database = storage.read().clone();
    if let Some(database) = database {
        if let Err(error) = database.upsert_session(session).await {
            tracing::warn!(session_id = %session.id, error = %error, "failed to persist session");
        }
    }
}

async fn persist_permission(storage: &StorageHandle, permission: &AgentPermissionRequest) {
    let database = storage.read().clone();
    if let Some(database) = database {
        if let Err(error) = database.upsert_permission(permission).await {
            tracing::warn!(
                permission_id = %permission.id,
                error = %error,
                "failed to persist permission"
            );
        }
    }
}

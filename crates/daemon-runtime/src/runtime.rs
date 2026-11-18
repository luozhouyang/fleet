//! Unified runtime: session registry, lifecycle state machine, event dispatch
//! and provider selection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use fleet_core_common::{now, ApiError};
use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentMessage, AgentPermissionRequest, AgentQuestionRequest,
    AgentSession, AgentStartConfig, AgentToolCall, AgentTurn, MessageCompletedPayload, MessageRole,
    PermissionDecision, PermissionEventPayload, PermissionStatus, ProviderInfo, ProviderKind,
    QuestionDecision, QuestionEventPayload, QuestionStatus, SendMessageRequest,
    SessionErrorPayload, SessionStatus, StatusChangedPayload, ToolCallStatus, ToolStatusPayload,
    TurnStatus,
};
use fleet_core_protocol::rpc::SessionDetail;
use fleet_daemon_storage::DaemonStorage;
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::agent::{AgentDefinition, AgentLaunchConfig, AgentRegistry};
use crate::driver::legacy::LegacyProviderDriver;
use crate::driver::{
    resolve_driver, AgentDriver, AgentInstanceHandle, AgentPrompt, DriverRegistry, ImageContent,
    PromptAcceptance, PromptContent,
};
use crate::error::{Result, RuntimeError};
use crate::event::{
    AgentEventEmitter, DispatchQueues, DispatchReceivers, PendingPermission, PendingQuestion,
    PermissionMap, ProcessedAgentEvent, QuestionMap,
};
use crate::instance::{AgentInstance, InstanceManager};
use crate::provider::{AgentProvider, ProviderRegistry};
use crate::session::SessionHandle;

type StorageHandle = Arc<RwLock<Option<Arc<DaemonStorage>>>>;

/// Snapshot of the runtime event pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventPipelineStats {
    /// State-lane events waiting to be dispatched.
    pub state_depth: usize,
    /// Live-only delta events waiting to be dispatched.
    pub delta_depth: usize,
    /// State events that did not fit the main lane and wait in the overflow
    /// lane.
    pub overflow_depth: usize,
    /// Ephemeral deltas dropped because the delta lane stayed full.
    pub dropped_ephemeral: u64,
    /// Durable events the store permanently rejected.
    pub rejected_events: u64,
}

/// Unified agent runtime.
pub struct AgentRuntime {
    /// Legacy provider registry, kept for provider catalog compatibility.
    legacy_registry: Option<ProviderRegistry>,
    agents: AgentRegistry,
    drivers: DriverRegistry,
    instances: Arc<InstanceManager>,
    sessions: Arc<DashMap<String, Arc<SessionHandle>>>,
    events: broadcast::Sender<AgentEvent>,
    dispatch: DispatchQueues,
    processed: broadcast::Sender<ProcessedAgentEvent>,
    storage: StorageHandle,
    /// Set while the durable outbox cannot be written. Retries continue and
    /// the health endpoint reports the degradation.
    storage_degraded: Arc<AtomicBool>,
}

impl AgentRuntime {
    /// Create a runtime from a legacy provider registry.
    ///
    /// Every provider is wrapped as a [`LegacyProviderDriver`] and gets a
    /// definition with the same id, so existing callers keep working while the
    /// runtime itself is driver-based.
    ///
    /// `event_capacity` is the broadcast buffer size; values below one are
    /// clamped to one.
    pub fn new(registry: ProviderRegistry, event_capacity: usize) -> Self {
        let mut agents = AgentRegistry::new();
        let mut drivers = DriverRegistry::new();
        for provider in registry.all() {
            let definition = definition_from_provider(provider.as_ref());
            let id = definition.id.clone();
            if let Err(error) = agents.register(definition) {
                tracing::error!(agent = %id, error = %error, "failed to register legacy agent definition");
                continue;
            }
            if let Err(error) = drivers.register(Arc::new(LegacyProviderDriver::new(provider))) {
                tracing::error!(agent = %id, error = %error, "failed to register legacy driver");
            }
        }
        Self::build(
            Some(registry),
            agents,
            drivers,
            event_capacity,
            Arc::new(InstanceManager::new()),
        )
    }

    /// Create a runtime from explicit agent and driver registries.
    ///
    /// This is the entry point used by the daemon after it registers built-in
    /// drivers and loads `[agents.*]` definitions.
    pub fn from_components(
        agents: AgentRegistry,
        drivers: DriverRegistry,
        event_capacity: usize,
    ) -> Self {
        Self::build(
            None,
            agents,
            drivers,
            event_capacity,
            Arc::new(InstanceManager::new()),
        )
    }

    fn build(
        legacy_registry: Option<ProviderRegistry>,
        agents: AgentRegistry,
        drivers: DriverRegistry,
        event_capacity: usize,
        instances: Arc<InstanceManager>,
    ) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        let (processed, _) = broadcast::channel(event_capacity.max(1));
        let (dispatch, dispatch_receivers) = DispatchQueues::new();
        let sessions: Arc<DashMap<String, Arc<SessionHandle>>> = Arc::new(DashMap::new());
        let storage: StorageHandle = Arc::new(RwLock::new(None));
        let storage_degraded = Arc::new(AtomicBool::new(false));

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let dispatch_sessions = Arc::clone(&sessions);
            let dispatch_storage = Arc::clone(&storage);
            let dispatch_instances = Arc::clone(&instances);
            let dispatch_processed = processed.clone();
            let dispatch_queues = dispatch.clone();
            let dispatch_degraded = Arc::clone(&storage_degraded);
            handle.spawn(dispatch_events(
                dispatch_sessions,
                dispatch_storage,
                dispatch_instances,
                dispatch_processed,
                dispatch_queues,
                dispatch_receivers,
                dispatch_degraded,
            ));
        } else {
            drop(dispatch_receivers);
            tracing::warn!(
                "AgentRuntime created outside a Tokio runtime; event state tracking is inactive"
            );
        }

        Self {
            legacy_registry,
            agents,
            drivers,
            instances,
            sessions,
            events,
            dispatch,
            processed,
            storage,
            storage_degraded,
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
            // Older records only have `provider`. Derive the agent/driver
            // metadata so the catalog and diagnostics keep working without
            // rewriting history.
            if session.metadata.get("agent").is_none() {
                let (definition_id, driver_id) = match self.agents.get(&session.provider) {
                    Some(definition) => (definition.id.clone(), definition.driver_id.clone()),
                    None => (session.provider.clone(), session.provider.clone()),
                };
                session.metadata["agent"] = json!({
                    "definition_id": definition_id,
                    "driver_id": driver_id,
                });
                session.metadata["driver_id"] = json!(driver_id);
            }

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

    /// Descriptors of all registered agents.
    ///
    /// This keeps the legacy `agent.providers.list` wire shape. For runtimes
    /// built from a [`ProviderRegistry`] the provider descriptors are returned
    /// verbatim; for driver-based runtimes the definitions are projected into
    /// the same shape.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        if let Some(registry) = &self.legacy_registry {
            return registry.infos();
        }
        let mut infos: Vec<ProviderInfo> = self
            .agents
            .list()
            .iter()
            .map(|definition| {
                let driver = self.drivers.get(&definition.driver_id);
                // Native wrappers keep the legacy adapter's descriptor so the
                // provider list still reports server-backed availability,
                // versions and models. Without a driver the definition cannot
                // start, so it must not be advertised as available.
                let provider = driver.as_ref().and_then(|driver| driver.provider_info());
                let capabilities = driver
                    .as_ref()
                    .map(|driver| driver.capabilities())
                    .unwrap_or_else(|| definition.declared_capabilities.clone());
                let available = definition.enabled
                    && driver.is_some()
                    && provider
                        .as_ref()
                        .map(|info| info.available)
                        .unwrap_or_else(|| executable_available(definition));
                ProviderInfo {
                    id: definition.id.clone(),
                    kind: provider
                        .as_ref()
                        .map(|info| info.kind)
                        .unwrap_or_else(|| provider_kind(definition)),
                    display_name: definition.name.clone(),
                    version: provider.as_ref().and_then(|info| info.version.clone()),
                    available,
                    capabilities,
                    models: provider.map(|info| info.models).unwrap_or_default(),
                }
            })
            .collect();
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    /// Descriptors of every registered driver.
    pub fn drivers(&self) -> Vec<crate::driver::DriverInfo> {
        self.drivers.list()
    }

    /// Definitions known to the runtime.
    pub fn agents(&self) -> Vec<Arc<AgentDefinition>> {
        self.agents.list()
    }

    /// Catalog view of every agent definition and its driver.
    pub fn agent_catalog(&self) -> Vec<crate::agent::AgentCatalogEntry> {
        self.agents
            .list()
            .iter()
            .map(|definition| {
                let driver = self.drivers.get(&definition.driver_id);
                crate::agent::catalog::entry(definition, driver.as_ref())
            })
            .collect()
    }

    /// Snapshot of tracked agent instances.
    pub fn instances(&self) -> Vec<AgentInstance> {
        self.instances.list()
    }

    /// Probe one agent definition without creating a session.
    pub async fn probe_agent(
        &self,
        definition_id: &str,
    ) -> Result<crate::driver::DriverProbeResult> {
        let definition = self
            .agents
            .get(definition_id)
            .ok_or_else(|| RuntimeError::AgentNotFound(definition_id.to_string()))?;
        let driver = resolve_driver(&definition, &self.drivers)?;
        driver.probe(&definition).await
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

    /// Whether the durable event outbox is currently failing to persist.
    ///
    /// While degraded, the dispatcher retains events and retries with
    /// exponential backoff instead of downgrading them to live-only events.
    pub fn is_storage_degraded(&self) -> bool {
        self.storage_degraded.load(Ordering::SeqCst)
    }

    /// Observability snapshot of the dispatcher queues.
    pub fn event_pipeline_stats(&self) -> EventPipelineStats {
        EventPipelineStats {
            state_depth: self.dispatch.state_depth(),
            delta_depth: self.dispatch.delta_depth(),
            overflow_depth: self.dispatch.overflow_depth(),
            dropped_ephemeral: self.dispatch.dropped_ephemeral(),
            rejected_events: self.dispatch.rejected_events(),
        }
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
    ///
    /// `agent_id` is an agent definition id. Legacy provider ids keep working
    /// because [`Self::new`](AgentRuntime::new) registers one definition per
    /// provider under the same id.
    pub async fn start_session(
        &self,
        machine_id: &str,
        agent_id: &str,
        mut config: AgentStartConfig,
    ) -> Result<AgentSession> {
        let definition = self
            .agents
            .get(agent_id)
            .ok_or_else(|| RuntimeError::AgentNotFound(agent_id.to_string()))?;
        let driver = resolve_driver(&definition, &self.drivers)?;
        config.cwd = resolve_definition_cwd(&definition, &config)?;
        merge_launch_env(&definition, &mut config);

        // Probes are advisory before start: some adapters become available
        // between the probe and the spawn, and the authoritative failure is
        // reported by `start` with its native error. The probe result is
        // persisted for diagnostics.
        let probe = match driver.probe_launch(&definition).await {
            Ok(probe) => {
                if !probe.available {
                    tracing::warn!(
                        agent = %definition.id,
                        driver = %driver.driver_id(),
                        diagnostics = ?probe.diagnostics,
                        "agent probe reports the agent as unavailable; attempting start anyway"
                    );
                }
                Some(probe)
            }
            Err(error) => {
                tracing::warn!(
                    agent = %definition.id,
                    driver = %driver.driver_id(),
                    error = %error,
                    "agent probe failed; attempting start anyway"
                );
                None
            }
        };

        let mut session = AgentSession::new(
            machine_id,
            definition.id.clone(),
            definition_agent_type(&definition),
            config.cwd.clone(),
        );
        session.workspace_id = config.workspace_id.clone();
        session.title = config.title.clone();
        session.status = SessionStatus::Creating;

        let permissions: PermissionMap = Arc::new(Mutex::new(HashMap::new()));
        let questions: QuestionMap = Arc::new(Mutex::new(HashMap::new()));
        let emitter = AgentEventEmitter::new(
            session.id.clone(),
            definition.id.clone(),
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
        // event observed by clients is the driver's `Running`.
        let starting = {
            let mut guard = handle.session.write().await;
            guard.status = SessionStatus::Starting;
            guard.updated_at = now();
            guard.clone()
        };
        self.persist_session(&starting).await;

        let instance_id = fleet_core_common::new_id("instance");
        self.instances.create(AgentInstance::starting(
            instance_id.clone(),
            definition.id.clone(),
            driver.driver_id().to_string(),
            session.id.clone(),
        ));

        let ctx = crate::driver::DriverContext {
            machine_id: machine_id.to_string(),
            instance_id: instance_id.clone(),
            session: starting,
            emitter: emitter.clone(),
        };

        match driver.start(ctx, &definition, config).await {
            Ok(started) => {
                let metadata = started.metadata.clone();
                self.instances
                    .mark_ready(&instance_id, started.process_id, metadata.clone());
                {
                    let mut guard = handle.session.write().await;
                    if !started.session.metadata.is_null() {
                        guard.metadata = started.session.metadata.clone();
                    }
                    merge_metadata(&mut guard.metadata, &metadata);
                    if started.session.title.is_some() {
                        guard.title = started.session.title.clone();
                    }
                    guard.agent_type = started.session.agent_type.clone();
                    apply_driver_metadata(
                        &mut guard.metadata,
                        &definition,
                        driver.driver_id(),
                        &instance_id,
                        probe.as_ref(),
                    );
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
                self.instances
                    .mark_failed(&instance_id, json!({"error": error.to_string()}));
                self.instances.remove(&instance_id);
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
                    json!({"reason": "agent start failed", "error": api}),
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
        let status = handle.session.read().await.status;
        if !matches!(
            status,
            SessionStatus::Running | SessionStatus::WaitingForInput
        ) {
            return Err(RuntimeError::InvalidState(format!(
                "cannot send a message while session is {status}"
            )));
        }
        let (driver, instance) = self.live_agent(session_id)?;
        let capabilities = driver.instance_capabilities(&instance);
        let mut prompt = build_prompt(&req, capabilities.images, driver.driver_id())?;
        let content = prompt.text_content();

        let mut turn = AgentTurn::new(session_id);
        turn.status = TurnStatus::Running;
        turn.started_at = now();
        // Admission is atomic: the in-flight check and the insertion happen in
        // one critical section, so concurrent prompts cannot start two turns.
        handle.try_begin_turn(turn.clone())?;
        // An interrupt may be delivered while the prompt is in flight. The
        // epoch tells the success path whether the cancel possibly reached
        // the agent before this prompt.
        let interrupt_epoch = handle.interrupt_epoch();

        if status == SessionStatus::WaitingForInput {
            let _ = self
                .transition(&handle, SessionStatus::Running, None, false)
                .await;
        }

        let user_message = AgentMessage::new(
            session_id,
            Some(turn.id.clone()),
            MessageRole::User,
            content.clone(),
        );
        handle.record_message(user_message.clone());
        prompt.metadata = json!({
            "fleet_message_id": user_message.id,
            "turn_id": turn.id,
        });

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

        match driver.send_message(&instance, prompt).await {
            Ok(acceptance) => {
                if handle.interrupt_epoch() != interrupt_epoch {
                    // The user interrupted while the prompt was being handed
                    // to the agent. The earlier cancel may have been sent
                    // before the prompt, so cancel again and close our turn;
                    // otherwise a prompt dispatched after an interrupt could
                    // execute unattended.
                    if let Err(error) = driver.interrupt(&instance).await {
                        tracing::warn!(
                            session_id,
                            error = %error,
                            "failed to cancel a prompt dispatched after an interrupt"
                        );
                    }
                    if let Some(updated) =
                        handle.finish_turn(Some(turn.id.as_str()), TurnStatus::Interrupted, None)
                    {
                        handle.emitter.emit_with(
                            AgentEventType::TurnInterrupted,
                            serde_json::to_value(&updated).unwrap_or_default(),
                            Some(turn.id.clone()),
                            None,
                            None,
                        );
                    }
                    mark_instance_idle_if_finished(&handle, &self.instances, session_id);
                    return Ok(());
                }
                self.instances.mark_busy(&instance.instance_id);
                let accepted = acceptance.is_accepted();
                let native_message_id = acceptance.native_message_id().map(str::to_string);
                let reason = match &acceptance {
                    PromptAcceptance::AcceptanceUnknown { reason, .. } => Some(reason.clone()),
                    PromptAcceptance::Accepted { .. } => None,
                };
                if native_message_id.is_some() || !accepted {
                    let updated = {
                        let mut guard = handle.session.write().await;
                        if !guard.metadata.is_object() {
                            guard.metadata = json!({});
                        }
                        guard.metadata["last_prompt"] = json!({
                            "native_message_id": native_message_id,
                            "accepted_at": acceptance.accepted_at(),
                            "accepted": accepted,
                            "reason": reason,
                        });
                        guard.updated_at = now();
                        guard.clone()
                    };
                    self.persist_session(&updated).await;
                }
                if !accepted {
                    // The prompt may or may not have reached the agent. Do
                    // not resend; watch for evidence of agent work and fail
                    // the turn if none arrives.
                    spawn_prompt_acceptance_watchdog(
                        Arc::clone(&handle),
                        turn.id.clone(),
                        PROMPT_ACCEPTANCE_WATCHDOG,
                    );
                }
            }
            Err(error) => {
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
        let (driver, instance) = self.live_agent(session_id)?;
        // Record the interrupt before contacting the agent so a prompt
        // dispatched concurrently observes the new epoch.
        handle.request_interrupt();
        driver.interrupt(&instance).await?;
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
        if !matches!(status, SessionStatus::Interrupted | SessionStatus::Failed) {
            return Err(RuntimeError::InvalidState(format!(
                "cannot resume a session that is {status}"
            )));
        }
        let (driver, instance) = self.live_agent(session_id)?;
        if !driver.capabilities().resume {
            return Err(RuntimeError::Unsupported(format!(
                "provider `{}` does not support resume",
                driver.driver_id()
            )));
        }
        let session_snapshot = handle.session.read().await.clone();
        let resumed = driver.resume(&instance, &session_snapshot).await?;
        {
            let mut guard = handle.session.write().await;
            guard.last_error = None;
        }
        if resumed.native_session_id.is_some() || resumed.protocol_version.is_some() {
            let updated = {
                let mut guard = handle.session.write().await;
                if !guard.metadata.is_object() {
                    guard.metadata = json!({});
                }
                if let Some(native) = &resumed.native_session_id {
                    guard.metadata["provider_session_id"] = json!(native);
                    guard.metadata["native"] = json!({ "session_id": native });
                }
                if let Some(version) = resumed.protocol_version {
                    guard.metadata["protocol"] = json!({
                        "name": "acp",
                        "version": version,
                    });
                }
                guard.updated_at = now();
                guard.clone()
            };
            self.persist_session(&updated).await;
        }
        let _ = self
            .transition(&handle, SessionStatus::Running, Some("resumed"), false)
            .await;
        Ok(())
    }

    /// Shut down a session. The session record remains queryable in its
    /// terminal state.
    pub async fn shutdown_session(&self, session_id: &str) -> Result<()> {
        let handle = self.session_handle(session_id)?;
        let mut shutdown_error = None;
        if !restored_session(&handle).await {
            if let Some((driver, instance)) = self.live_agent_opt(session_id) {
                if let Err(error) = driver.shutdown(&instance).await {
                    if !matches!(error, RuntimeError::SessionNotFound(_)) {
                        // Keep tearing down: a failed provider shutdown must
                        // not leave the session and instance stuck in Running.
                        shutdown_error = Some(error);
                    }
                }
                self.instances.stop(&instance.instance_id);
            }
        }
        handle.cancel.cancel();
        // A prompt body that is still dispatching must not survive shutdown.
        handle.request_interrupt();
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
        match shutdown_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
                if let Some((driver, instance)) = self.live_agent_opt(session_id) {
                    if let Err(error) = driver.shutdown(&instance).await {
                        if !matches!(error, RuntimeError::SessionNotFound(_)) {
                            tracing::warn!(session_id, error = %error, "driver shutdown failed during delete");
                        }
                    }
                    self.instances.stop(&instance.instance_id);
                }
            }
            handle.cancel.cancel();
            handle.request_interrupt();
            cancel_pending_permissions(&handle, PermissionStatus::Cancelled, &self.storage).await;
            cancel_pending_questions(&handle, QuestionStatus::Cancelled, &self.storage).await;
        }
        self.instances.remove_by_session(session_id);

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
        self.dispatch.send(event.clone());
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
            // No runtime-owned sender: the driver must deliver the decision
            // over its native approval channel.
            let (driver, instance) = self.live_agent(&permission.session_id)?;
            driver
                .respond_permission(&instance, decision.clone())
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
            let (driver, instance) = self.live_agent(&question.session_id)?;
            driver.respond_question(&instance, decision.clone()).await?;
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

    /// Resolve the running driver and instance for a session.
    fn live_agent(&self, session_id: &str) -> Result<(Arc<dyn AgentDriver>, AgentInstanceHandle)> {
        self.live_agent_opt(session_id).ok_or_else(|| {
            RuntimeError::ProviderUnavailable(format!(
                "session `{session_id}` has no live agent instance"
            ))
        })
    }

    /// Resolve the running driver and instance, when one exists.
    fn live_agent_opt(
        &self,
        session_id: &str,
    ) -> Option<(Arc<dyn AgentDriver>, AgentInstanceHandle)> {
        let record = self.instances.get_by_session(session_id)?;
        let handle = AgentInstanceHandle {
            instance_id: record.id,
            definition_id: record.definition_id,
            driver_id: record.driver_id.clone(),
            session_id: record.session_id,
            process_id: record.process_id,
            metadata: record.metadata,
        };
        let driver = self.drivers.get(&record.driver_id)?;
        Some((driver, handle))
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

fn build_prompt(
    request: &SendMessageRequest,
    accepts_images: bool,
    driver_id: &str,
) -> Result<AgentPrompt> {
    if request.attachments.is_empty() {
        return Ok(AgentPrompt::text(request.content.clone()));
    }
    if !accepts_images {
        return Err(RuntimeError::Unsupported(format!(
            "provider `{driver_id}` does not accept message attachments"
        )));
    }
    let mut content = Vec::with_capacity(request.attachments.len() + 1);
    if !request.content.is_empty() {
        content.push(PromptContent::Text(request.content.clone()));
    }
    for attachment in &request.attachments {
        content.push(PromptContent::Image(ImageContent {
            data: attachment.data_base64.clone(),
            mime_type: attachment
                .mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            name: Some(attachment.name.clone()),
        }));
    }
    Ok(AgentPrompt {
        content,
        metadata: Value::Null,
    })
}

/// Build a definition for a legacy provider so that the provider id keeps
/// working as an agent id.
fn definition_from_provider(provider: &dyn AgentProvider) -> AgentDefinition {
    let info = provider.info();
    let executable = provider.executable().unwrap_or(provider.provider_id());
    let mut definition = AgentDefinition::process(
        info.id.clone(),
        info.display_name.clone(),
        provider.provider_id(),
        crate::agent::ProcessLaunchConfig::new(executable),
    );
    definition.declared_capabilities = provider.capabilities();
    definition.source = crate::agent::AgentDefinitionSource::Builtin;
    definition.metadata = json!({
        "provider_kind": info.kind,
        "legacy_provider": true,
        "agent_type": provider.agent_type(),
    });
    definition
}

fn provider_kind(definition: &AgentDefinition) -> ProviderKind {
    definition
        .metadata
        .get("provider_kind")
        .and_then(|value| serde_json::from_value::<ProviderKind>(value.clone()).ok())
        .unwrap_or(ProviderKind::Generic)
}

fn definition_agent_type(definition: &AgentDefinition) -> String {
    definition
        .metadata
        .get("agent_type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| definition.id.clone())
}

/// Transition the session's instance back to `Ready` once no turn is active.
fn mark_instance_idle_if_finished(
    handle: &SessionHandle,
    instances: &InstanceManager,
    session_id: &str,
) {
    if handle.in_flight.lock().is_empty() {
        instances.mark_session_idle(session_id);
    }
}

/// Resolve the session working directory from the definition's `cwd_policy`.
fn resolve_definition_cwd(
    definition: &AgentDefinition,
    config: &AgentStartConfig,
) -> Result<String> {
    let resolved = definition
        .resolve_cwd(&config.cwd, config.workspace_root.as_deref())
        .unwrap_or_else(|| config.cwd.clone());
    if resolved.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "sessions require a working directory".to_string(),
        ));
    }
    Ok(resolved)
}

/// Merge launch environment into the session environment.
///
/// Session-provided values win, so an admin can override a definition default
/// for one session without editing the daemon configuration.
fn merge_launch_env(definition: &AgentDefinition, config: &mut AgentStartConfig) {
    let AgentLaunchConfig::Process(process) = &definition.launch else {
        return;
    };
    for (key, value) in &process.env {
        config
            .env
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

fn executable_available(definition: &AgentDefinition) -> bool {
    crate::agent::catalog::executable_available(definition)
}

/// Merge object metadata into an existing object value.
fn merge_metadata(target: &mut Value, incoming: &Value) {
    if !target.is_object() {
        *target = json!({});
    }
    let Value::Object(incoming) = incoming else {
        return;
    };
    if let Value::Object(target) = target {
        for (key, value) in incoming {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn apply_driver_metadata(
    metadata: &mut Value,
    definition: &AgentDefinition,
    driver_id: &str,
    instance_id: &str,
    probe: Option<&crate::driver::DriverProbeResult>,
) {
    if !metadata.is_object() {
        *metadata = json!({});
    }
    metadata["agent"] = json!({
        "definition_id": definition.id,
        "driver_id": driver_id,
        "instance_id": instance_id,
    });
    metadata["definition_snapshot"] = definition.snapshot();
    metadata["launch_snapshot"] = definition.snapshot()["launch_snapshot"].clone();
    if let Some(probe) = probe {
        metadata["probe"] = serde_json::to_value(probe).unwrap_or(Value::Null);
    }
    for (key, value) in definition_metadata_pairs(definition) {
        metadata[&key] = value;
    }
}

fn definition_metadata_pairs(definition: &AgentDefinition) -> Vec<(String, Value)> {
    match &definition.metadata {
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| key.as_str() != "provider_kind")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

async fn dispatch_events(
    sessions: Arc<DashMap<String, Arc<SessionHandle>>>,
    storage: StorageHandle,
    instances: Arc<InstanceManager>,
    processed: broadcast::Sender<ProcessedAgentEvent>,
    queues: DispatchQueues,
    receivers: DispatchReceivers,
    storage_degraded: Arc<AtomicBool>,
) {
    let DispatchReceivers {
        mut state,
        mut delta,
        mut overflow,
    } = receivers;
    // At most this many queued deltas are dispatched before the next state
    // event so a sustained token stream cannot starve the state machine.
    let mut delta_budget = DELTA_BURST;
    // Last dispatched sequence per session. Deltas that lost a race against a
    // newer state event are dropped instead of mutating a completed message.
    let mut last_sequences: HashMap<String, u64> = HashMap::new();
    while let Some(mut event) = next_dispatched_event(
        &queues,
        &mut state,
        &mut delta,
        &mut overflow,
        &mut delta_budget,
    )
    .await
    {
        if matches!(
            event.event_type,
            AgentEventType::MessageDelta | AgentEventType::ToolInputDelta
        ) && last_sequences
            .get(&event.session_id)
            .is_some_and(|sequence| event.sequence <= *sequence)
        {
            queues.note_dropped_ephemeral();
            continue;
        }
        last_sequences.insert(event.session_id.clone(), event.sequence);
        if event.event_type == AgentEventType::SessionDeleted {
            last_sequences.remove(&event.session_id);
        }
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
                        let error = payload.error;
                        apply_error(&handle, error.clone()).await;
                        if let Some(instance) = instances.get_by_session(&event.session_id) {
                            instances.mark_failed(&instance.id, json!({ "error": error.clone() }));
                        }
                        // A terminal session error means no live agent owns
                        // work any more. Close the open transcript state so a
                        // restart or connection loss cannot leave turns
                        // running forever.
                        let active: Vec<String> = handle
                            .in_flight
                            .lock()
                            .iter()
                            .map(|in_flight| in_flight.turn_id.clone())
                            .collect();
                        for turn_id in active {
                            if let Some(updated) = handle.finish_turn(
                                Some(turn_id.as_str()),
                                TurnStatus::Failed,
                                Some(error.clone()),
                            ) {
                                handle.emitter.emit_with(
                                    AgentEventType::TurnFailed,
                                    json!({ "error": &error, "turn": updated }),
                                    Some(turn_id),
                                    None,
                                    None,
                                );
                            }
                        }
                        let running: Vec<(String, Option<String>)> = handle
                            .tools
                            .lock()
                            .iter()
                            .filter(|tool| {
                                matches!(
                                    tool.status,
                                    ToolCallStatus::Pending | ToolCallStatus::Running
                                )
                            })
                            .map(|tool| (tool.id.clone(), tool.turn_id.clone()))
                            .collect();
                        for (tool_id, turn_id) in running {
                            if let Some(tool) = handle.finish_tool(
                                &tool_id,
                                ToolCallStatus::Cancelled,
                                Some("the agent connection was lost".to_string()),
                            ) {
                                handle.emitter.emit_with(
                                    AgentEventType::ToolFailed,
                                    json!({
                                        "tool": tool,
                                        "error": "the agent connection was lost",
                                    }),
                                    turn_id,
                                    None,
                                    Some(tool_id),
                                );
                            }
                        }
                        cancel_pending_permissions(&handle, PermissionStatus::Cancelled, &storage)
                            .await;
                        cancel_pending_questions(&handle, QuestionStatus::Cancelled, &storage)
                            .await;
                    }
                }
                AgentEventType::TurnStarted => {
                    if let Ok(turn) = serde_json::from_value::<AgentTurn>(event.payload.clone()) {
                        handle.upsert_turn(turn);
                    }
                }
                AgentEventType::TurnCompleted => {
                    handle.finish_turn(event.turn_id.as_deref(), TurnStatus::Completed, None);
                    mark_instance_idle_if_finished(&handle, &instances, &event.session_id);
                }
                AgentEventType::TurnInterrupted => {
                    handle.finish_turn(event.turn_id.as_deref(), TurnStatus::Interrupted, None);
                    mark_instance_idle_if_finished(&handle, &instances, &event.session_id);
                }
                AgentEventType::TurnFailed => {
                    let error = event
                        .payload
                        .get("error")
                        .and_then(|value| serde_json::from_value::<ApiError>(value.clone()).ok());
                    handle.finish_turn(event.turn_id.as_deref(), TurnStatus::Failed, error);
                    mark_instance_idle_if_finished(&handle, &instances, &event.session_id);
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
        // deltas are ignored by the store.
        //
        // A durable event must never be downgraded to a live-only event just
        // because the database is temporarily unavailable: that would let the
        // admin's replay window lose a state transition. Retry with
        // exponential backoff and hold the event at the head of the
        // dispatcher until it is persisted. While retrying, the queue depth
        // rises and the health endpoint reports degraded storage.
        let database = storage.read().clone();
        let cursor = match database {
            Some(database) => {
                let mut attempt: u32 = 0;
                loop {
                    match database.record_event(&event).await {
                        Ok(cursor) => {
                            if storage_degraded.swap(false, Ordering::SeqCst) {
                                tracing::info!(
                                    "daemon outbox recovered; durable event recording resumed"
                                );
                            }
                            break cursor;
                        }
                        Err(error) if !error.is_retryable() => {
                            // A deterministic rejection cannot be fixed by
                            // retrying. Holding the dispatcher would stall
                            // every later event behind one poison record, so
                            // surface the loss, count it, and deliver the
                            // event live-only (no cursor, therefore not
                            // acknowledgeable).
                            queues.note_store_rejection();
                            tracing::error!(
                                session_id = %event.session_id,
                                event_id = %event.event_id,
                                error = %error,
                                "daemon outbox permanently rejected a durable event"
                            );
                            break None;
                        }
                        Err(error) => {
                            attempt = attempt.saturating_add(1);
                            storage_degraded.store(true, Ordering::SeqCst);
                            let delay = outbox_retry_delay(attempt);
                            if attempt <= 5 || attempt.is_multiple_of(20) {
                                tracing::warn!(
                                    session_id = %event.session_id,
                                    event_id = %event.event_id,
                                    error = %error,
                                    attempt,
                                    retry_in_ms = delay.as_millis() as u64,
                                    "failed to record agent event in the daemon outbox; retaining event for retry"
                                );
                            }
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
            None => None,
        };
        let _ = processed.send(ProcessedAgentEvent { event, cursor });
    }
}

/// Exponential backoff for durable outbox writes, capped at five seconds.
fn outbox_retry_delay(attempt: u32) -> std::time::Duration {
    let exponent = attempt.saturating_sub(1).min(8);
    let millis = 20u64.saturating_mul(1u64 << exponent).min(5_000);
    std::time::Duration::from_millis(millis)
}

/// How long an unconfirmed prompt may stay silent before the turn is failed.
///
/// ACP v2 may accept the prompt without its response reaching Fleet. The
/// session then must not stay `running` forever: any event for the session
/// cancels this watchdog, and silence past the deadline fails the turn.
const PROMPT_ACCEPTANCE_WATCHDOG: std::time::Duration = std::time::Duration::from_secs(120);

/// Fail a turn whose prompt acceptance was never confirmed and that produced
/// no agent activity.
///
/// Emitting `session.error` is enough: the dispatcher fails every in-flight
/// turn, marks open tools and permissions terminal, and moves the session to
/// `failed` so the UI offers a recovery path.
fn spawn_prompt_acceptance_watchdog(
    handle: Arc<SessionHandle>,
    turn_id: String,
    watchdog: std::time::Duration,
) {
    // Record the activity baseline synchronously so an update emitted before
    // the task is first polled still counts as agent activity.
    let start_sequence = handle.emitter.event_sequence();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + watchdog;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            tokio::time::sleep((deadline - now).min(std::time::Duration::from_secs(1))).await;
            if handle.cancel.is_cancelled() {
                // The session is shutting down or was deleted.
                return;
            }
            if !handle
                .in_flight
                .lock()
                .iter()
                .any(|in_flight| in_flight.turn_id == turn_id)
            {
                // The turn finished or was interrupted; the event stream won.
                return;
            }
            if handle.emitter.event_sequence() > start_sequence {
                // The agent produced updates: it accepted the prompt and the
                // normal turn lifecycle owns completion now.
                return;
            }
        }
        let error = ApiError::new(
            "prompt_acceptance_unknown",
            "the agent did not confirm the prompt and produced no updates; the turn was abandoned",
        );
        handle.emitter.emit(
            AgentEventType::SessionError,
            serde_json::to_value(SessionErrorPayload { error }).unwrap_or_default(),
        );
    });
}

/// How many queued deltas may be dispatched between two state events.
const DELTA_BURST: usize = 64;

/// Pick the next event to dispatch.
///
/// Deltas queued before a state event are dispatched first, up to a bounded
/// burst, so completion does not overtake the text of the same message.
/// State events themselves stay ordered: the main lane is drained before the
/// overflow lane, and overflow producers only stop once the dispatcher
/// observed both lanes empty.
async fn next_dispatched_event(
    queues: &DispatchQueues,
    state: &mut mpsc::Receiver<AgentEvent>,
    delta: &mut mpsc::Receiver<AgentEvent>,
    overflow: &mut mpsc::UnboundedReceiver<AgentEvent>,
    delta_budget: &mut usize,
) -> Option<AgentEvent> {
    loop {
        // Drain queued deltas before a newer state event, bounded so a
        // sustained token stream cannot starve the state machine.
        if *delta_budget > 0 {
            if let Ok(event) = delta.try_recv() {
                *delta_budget = delta_budget.saturating_sub(1);
                return Some(event);
            }
        }
        // Older state events in the main lane first.
        match state.try_recv() {
            Ok(event) => {
                *delta_budget = DELTA_BURST;
                return Some(event);
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {}
        }
        // Overflow events are newer than anything left in the main lane.
        if let Ok(event) = overflow.try_recv() {
            queues.overflow_received();
            *delta_budget = DELTA_BURST;
            return Some(event);
        }
        if !queues.try_end_overflow() {
            // A producer queued another overflow event before the lane was
            // disabled; drain it before switching back to the main lane.
            continue;
        }
        // The overflow lane is disabled under the routing lock, so every
        // subsequent state event goes to the main lane. Re-check it once.
        if let Ok(event) = state.try_recv() {
            *delta_budget = DELTA_BURST;
            return Some(event);
        }
        break;
    }
    // No state event is pending; refill the budget and drain deltas.
    *delta_budget = DELTA_BURST;
    if let Ok(event) = delta.try_recv() {
        *delta_budget = delta_budget.saturating_sub(1);
        return Some(event);
    }
    tokio::select! {
        biased;
        Some(event) = overflow.recv() => {
            queues.overflow_received();
            *delta_budget = DELTA_BURST;
            Some(event)
        }
        Some(event) = state.recv() => {
            *delta_budget = DELTA_BURST;
            Some(event)
        }
        Some(event) = delta.recv() => {
            *delta_budget = delta_budget.saturating_sub(1);
            Some(event)
        }
        else => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{CwdPolicy, ProcessLaunchConfig};

    fn definition_with_launch(cwd_policy: CwdPolicy) -> AgentDefinition {
        let mut launch = ProcessLaunchConfig::new("agent");
        launch.cwd_policy = cwd_policy;
        launch
            .env
            .insert("FROM_LAUNCH".to_string(), "definition".to_string());
        launch
            .env
            .insert("SHARED".to_string(), "definition".to_string());
        AgentDefinition::process("agent", "Agent", "acp", launch)
    }

    #[test]
    fn cwd_policy_uses_the_registered_workspace_root() {
        let definition = definition_with_launch(CwdPolicy::WorkspaceRoot);
        let config = AgentStartConfig {
            cwd: "/session/subdir".to_string(),
            workspace_root: Some("/workspace".to_string()),
            ..AgentStartConfig::default()
        };
        assert_eq!(
            resolve_definition_cwd(&definition, &config).unwrap(),
            "/workspace"
        );
    }

    #[test]
    fn launch_environment_never_overrides_session_values() {
        let definition = definition_with_launch(CwdPolicy::Session);
        let mut config = AgentStartConfig {
            cwd: "/session".to_string(),
            env: [("SHARED".to_string(), "session".to_string())].into(),
            ..AgentStartConfig::default()
        };
        merge_launch_env(&definition, &mut config);
        assert_eq!(config.env["FROM_LAUNCH"], "definition");
        assert_eq!(config.env["SHARED"], "session");
    }

    #[test]
    fn definition_agent_type_prefers_metadata() {
        let definition = definition_with_launch(CwdPolicy::Session);
        assert_eq!(definition_agent_type(&definition), "agent");
        let mut definition = definition;
        definition.metadata = json!({"agent_type": "claude-code"});
        assert_eq!(definition_agent_type(&definition), "claude-code");
    }

    #[test]
    fn outbox_retry_backoff_is_exponential_and_capped() {
        use std::time::Duration;
        assert_eq!(outbox_retry_delay(1), Duration::from_millis(20));
        assert_eq!(outbox_retry_delay(2), Duration::from_millis(40));
        assert_eq!(outbox_retry_delay(3), Duration::from_millis(80));
        assert_eq!(outbox_retry_delay(8), Duration::from_millis(2_560));
        assert_eq!(outbox_retry_delay(20), Duration::from_millis(5_000));
    }

    fn watchdog_handle() -> Arc<SessionHandle> {
        let (events, _) = tokio::sync::broadcast::channel(64);
        let (dispatch, _receivers) = DispatchQueues::new();
        let permissions = Arc::new(Mutex::new(HashMap::new()));
        let questions = Arc::new(Mutex::new(HashMap::new()));
        let emitter = AgentEventEmitter::new(
            "session_watchdog",
            "mock",
            events,
            dispatch,
            Arc::clone(&permissions),
            Arc::clone(&questions),
        );
        Arc::new(SessionHandle::new(
            fleet_core_protocol::agent::AgentSession::new("machine_1", "mock", "mock", "/tmp"),
            emitter,
            permissions,
            questions,
        ))
    }

    fn admitted_turn(handle: &SessionHandle) -> AgentTurn {
        let mut turn = AgentTurn::new("session_watchdog");
        turn.status = TurnStatus::Running;
        handle.try_begin_turn(turn.clone()).unwrap();
        turn
    }

    #[tokio::test]
    async fn acceptance_watchdog_fails_a_silent_turn() {
        use std::time::Duration;

        let handle = watchdog_handle();
        let turn = admitted_turn(&handle);
        let mut events = handle.emitter.subscribe_for_test();
        spawn_prompt_acceptance_watchdog(
            Arc::clone(&handle),
            turn.id.clone(),
            Duration::from_millis(10),
        );

        let event = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match events.recv().await {
                    Ok(event) if event.event_type == AgentEventType::SessionError => {
                        return event;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => panic!("event bus closed"),
                }
            }
        })
        .await
        .expect("watchdog must report the unconfirmed turn");
        let payload: SessionErrorPayload = serde_json::from_value(event.payload).unwrap();
        assert_eq!(payload.error.code, "prompt_acceptance_unknown");
    }

    #[tokio::test]
    async fn acceptance_watchdog_stops_when_the_agent_produces_events() {
        use std::time::Duration;

        let handle = watchdog_handle();
        let turn = admitted_turn(&handle);
        let mut events = handle.emitter.subscribe_for_test();
        spawn_prompt_acceptance_watchdog(
            Arc::clone(&handle),
            turn.id.clone(),
            Duration::from_millis(80),
        );
        handle
            .emitter
            .emit(AgentEventType::MessageDelta, json!({"text": "working"}));

        tokio::time::sleep(Duration::from_millis(200)).await;
        while let Ok(event) = events.try_recv() {
            assert_ne!(
                event.event_type,
                AgentEventType::SessionError,
                "agent activity must cancel the acceptance watchdog"
            );
        }
        assert_eq!(handle.in_flight.lock().len(), 1);
    }

    /// Driver whose prompt dispatch blocks until the test releases it.
    struct BlockingDriver {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        interrupts: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AgentDriver for BlockingDriver {
        fn driver_id(&self) -> &str {
            "blocking"
        }

        fn info(&self) -> crate::driver::DriverInfo {
            crate::driver::DriverInfo {
                id: "blocking".to_string(),
                display_name: "Blocking".to_string(),
                protocol: crate::driver::DriverProtocol::Native,
                version: None,
            }
        }

        fn capabilities(&self) -> fleet_core_protocol::agent::AgentCapabilities {
            fleet_core_protocol::agent::AgentCapabilities::basic()
        }

        async fn probe(
            &self,
            definition: &AgentDefinition,
        ) -> Result<crate::driver::DriverProbeResult> {
            Ok(crate::driver::DriverProbeResult {
                agent_id: Some(definition.id.clone()),
                driver_id: self.driver_id().to_string(),
                protocol: crate::driver::DriverProtocol::Native,
                protocol_version: None,
                available: true,
                capabilities: crate::driver::ResolvedCapabilities::new(
                    definition.declared_capabilities.clone(),
                    self.capabilities(),
                    self.capabilities(),
                ),
                agent_info: None,
                diagnostics: Vec::new(),
            })
        }

        async fn start(
            &self,
            ctx: crate::driver::DriverContext,
            _definition: &AgentDefinition,
            _config: AgentStartConfig,
        ) -> Result<crate::driver::DriverStartResult> {
            let mut session = ctx.session;
            session.status = SessionStatus::Running;
            Ok(crate::driver::DriverStartResult::new(session))
        }

        async fn send_message(
            &self,
            _instance: &AgentInstanceHandle,
            _message: AgentPrompt,
        ) -> Result<PromptAcceptance> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(PromptAcceptance::now(None))
        }

        async fn interrupt(&self, _instance: &AgentInstanceHandle) -> Result<()> {
            self.interrupts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn resume(
            &self,
            _instance: &AgentInstanceHandle,
            _session: &AgentSession,
        ) -> Result<crate::driver::ResumeResult> {
            Ok(crate::driver::ResumeResult {
                resumed: true,
                native_session_id: None,
                protocol_version: None,
            })
        }

        async fn shutdown(&self, _instance: &AgentInstanceHandle) -> Result<()> {
            Ok(())
        }

        async fn respond_permission(
            &self,
            _instance: &AgentInstanceHandle,
            _decision: PermissionDecision,
        ) -> Result<()> {
            Ok(())
        }

        async fn respond_question(
            &self,
            _instance: &AgentInstanceHandle,
            _decision: QuestionDecision,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_interrupt_racing_a_prompt_dispatch_cancels_the_late_prompt() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let interrupts = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let mut agents = AgentRegistry::new();
        agents
            .register(AgentDefinition::process(
                "blocking",
                "Blocking",
                "blocking",
                ProcessLaunchConfig::new("blocking"),
            ))
            .unwrap();
        let mut drivers = DriverRegistry::new();
        drivers
            .register(Arc::new(BlockingDriver {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
                interrupts: Arc::clone(&interrupts),
            }))
            .unwrap();
        let runtime = AgentRuntime::from_components(agents, drivers, 64);

        let session = runtime
            .start_session(
                "machine_1",
                "blocking",
                AgentStartConfig {
                    cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                    ..AgentStartConfig::default()
                },
            )
            .await
            .expect("start session");

        let send = runtime.send_message(
            &session.id,
            SendMessageRequest {
                content: "go".to_string(),
                attachments: Vec::new(),
            },
        );
        let interrupt = async {
            entered.notified().await;
            runtime.interrupt(&session.id).await.expect("interrupt");
            release.notify_one();
        };
        let (sent, ()) = tokio::join!(send, interrupt);
        sent.expect("send_message");

        assert!(
            interrupts.load(Ordering::SeqCst) >= 2,
            "a prompt dispatched after an interrupt must be cancelled again"
        );
        let detail = runtime.session_detail(&session.id).await.expect("detail");
        assert_eq!(detail.session.status, SessionStatus::Interrupted);
        assert!(detail
            .turns
            .iter()
            .all(|turn| turn.status == TurnStatus::Interrupted));
    }
}

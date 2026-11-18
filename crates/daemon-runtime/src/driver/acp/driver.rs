//! The generic ACP driver.
//!
//! Any agent that speaks ACP is served by this driver. Per-agent differences
//! live in the [`AgentDefinition`](crate::agent::AgentDefinition): executable,
//! arguments, environment and working-directory policy. The driver itself
//! never mentions a vendor.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEventType, AgentSession, AgentStartConfig, PermissionDecision,
    PermissionStatus, QuestionDecision, QuestionStatus, SessionStatus,
};
use serde_json::{json, Value};

use crate::agent::AgentDefinition;
use crate::driver::capabilities::ResolvedCapabilities;
use crate::driver::context::DriverContext;
use crate::driver::instance::{AgentInstanceHandle, DriverStartResult};
use crate::driver::trait_def::{
    AgentDriver, AgentPrompt, DriverInfo, DriverProbeResult, DriverProtocol, PromptAcceptance,
    ResumeResult,
};
use crate::error::{Result, RuntimeError};

use super::config::{AcpDriverConfig, AcpProtocolVersion};
use super::connection::{self, SpawnedConnection, V2SpawnOutcome};
use super::mapper::common;
use super::process::{process_config, AcpProcessConfig};
use super::session::AcpSessionHandle;

/// Generic driver for Agent Client Protocol agents.
pub struct AcpDriver {
    config: AcpDriverConfig,
    sessions: DashMap<String, Arc<AcpSessionHandle>>,
}

impl AcpDriver {
    /// Create a driver with the given protocol policy and timeouts.
    pub fn new(config: AcpDriverConfig) -> Self {
        Self {
            config,
            sessions: DashMap::new(),
        }
    }

    /// Create a driver that only speaks ACP v1.
    pub fn v1_only() -> Self {
        Self::new(AcpDriverConfig::default())
    }

    /// The driver configuration.
    pub fn config(&self) -> &AcpDriverConfig {
        &self.config
    }

    fn session(&self, session_id: &str) -> Result<Arc<AcpSessionHandle>> {
        self.sessions
            .get(session_id)
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))
    }

    /// Executable and policy checks that do not spawn the agent.
    fn static_probe(&self, definition: &AgentDefinition) -> (bool, Vec<String>) {
        let mut diagnostics = Vec::new();
        let available = match definition.launch.process() {
            Some(process) => {
                if process.executable.trim().is_empty() {
                    diagnostics.push("agent definition has no executable".to_string());
                    false
                } else if crate::provider::binary_exists(&process.executable) {
                    true
                } else {
                    diagnostics.push(format!(
                        "ACP executable `{}` was not found",
                        process.executable
                    ));
                    false
                }
            }
            None => {
                diagnostics.push("agent definition does not use a process launch".to_string());
                false
            }
        };
        (available, diagnostics)
    }

    async fn start_connection(
        &self,
        process: &AcpProcessConfig,
        session: &Arc<AcpSessionHandle>,
    ) -> Result<SpawnedConnection> {
        let attempts = self.config.protocol_attempts();
        if attempts.is_empty() {
            return Err(RuntimeError::Unsupported(
                "ACP v2 is disabled by the feature flag but the protocol policy requires it"
                    .to_string(),
            ));
        }
        let mut last_error = None;
        for attempt in attempts {
            match attempt {
                AcpProtocolVersion::V1 => {
                    return connection::spawn_v1(
                        process.clone_for_retry(),
                        Arc::clone(session),
                        &self.config,
                    )
                    .await;
                }
                AcpProtocolVersion::V2 => {
                    match connection::spawn_v2(
                        process.clone_for_retry(),
                        Arc::clone(session),
                        &self.config,
                    )
                    .await?
                    {
                        V2SpawnOutcome::Ready(connection) => return Ok(connection),
                        V2SpawnOutcome::LowerVersion | V2SpawnOutcome::UnsupportedVersion(_) => {
                            if self.config.protocol_policy
                                == super::config::AcpProtocolPolicy::V2Only
                            {
                                return Err(RuntimeError::Unsupported(
                                    "the agent does not support ACP v2".to_string(),
                                ));
                            }
                            tracing::info!(
                                session_id = %session.fleet_session_id,
                                "agent did not negotiate ACP v2; reconnecting with v1"
                            );
                            last_error = Some(RuntimeError::Unsupported(
                                "agent selected ACP v1".to_string(),
                            ));
                        }
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            RuntimeError::Unsupported("no ACP protocol version could be negotiated".to_string())
        }))
    }
}

#[async_trait]
impl AgentDriver for AcpDriver {
    fn driver_id(&self) -> &str {
        "acp"
    }

    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "acp".to_string(),
            display_name: "Agent Client Protocol".to_string(),
            protocol: DriverProtocol::Acp,
            version: Some(self.config.client_version.clone()),
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            interrupt: true,
            resume: true,
            permission_approval: true,
            user_questions: true,
            structured_events: true,
            structured_diff: false,
            session_persistence: true,
            tool_calls: true,
            images: false,
        }
    }

    fn instance_capabilities(&self, instance: &AgentInstanceHandle) -> AgentCapabilities {
        self.sessions
            .get(&instance.session_id)
            .map(|session| session.capabilities.lock().clone())
            .unwrap_or_else(|| self.capabilities())
    }

    async fn probe_launch(&self, definition: &AgentDefinition) -> Result<DriverProbeResult> {
        let (available, diagnostics) = self.static_probe(definition);
        let driver_supported = self.capabilities();
        Ok(DriverProbeResult {
            agent_id: Some(definition.id.clone()),
            driver_id: "acp".to_string(),
            protocol: DriverProtocol::Acp,
            protocol_version: None,
            available,
            capabilities: ResolvedCapabilities::new(
                definition.declared_capabilities.clone(),
                driver_supported.clone(),
                driver_supported,
            ),
            agent_info: None,
            diagnostics,
        })
    }

    async fn probe(&self, definition: &AgentDefinition) -> Result<DriverProbeResult> {
        let (available, mut diagnostics) = self.static_probe(definition);
        if self.config.protocol_attempts().is_empty() {
            diagnostics.push(
                "ACP v2 is disabled by the feature flag but the policy requires v2".to_string(),
            );
        }
        let driver_supported = self.capabilities();
        let mut available = available && !self.config.protocol_attempts().is_empty();
        let mut protocol_version = None;
        let mut agent_info = None;
        let mut verified = driver_supported.clone();
        if available {
            // A real handshake is the only way to verify the protocol version
            // and the agent's capabilities. The probe uses a throwaway session
            // handle and never creates a native session.
            match process_config(definition, &std::collections::BTreeMap::new()) {
                Ok(process) => {
                    let (events, _) = tokio::sync::broadcast::channel(8);
                    let (dispatch, _) = tokio::sync::mpsc::unbounded_channel();
                    let permissions =
                        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
                    let questions =
                        Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
                    let emitter = crate::event::AgentEventEmitter::new(
                        format!("probe:{}", definition.id),
                        definition.id.clone(),
                        events,
                        dispatch,
                        permissions,
                        questions,
                    );
                    let session = AcpSessionHandle::new(
                        format!("probe:{}", definition.id),
                        "probe",
                        emitter,
                        self.config.permission_timeout,
                        self.config.question_timeout,
                    );
                    match self.start_connection(&process, &session).await {
                        Ok(spawned) => {
                            protocol_version = Some(spawned.protocol.number());
                            agent_info = Some(spawned.agent_info.clone());
                            verified = spawned.capabilities.clone();
                            let task = session.task.lock().take();
                            if let Some(task) = task {
                                connection::abort(task).await;
                            }
                        }
                        Err(error) => {
                            available = false;
                            diagnostics.push(format!("ACP handshake failed: {error}"));
                        }
                    }
                }
                Err(error) => {
                    available = false;
                    diagnostics.push(format!("invalid ACP process configuration: {error}"));
                }
            }
        }
        Ok(DriverProbeResult {
            agent_id: Some(definition.id.clone()),
            driver_id: "acp".to_string(),
            protocol: DriverProtocol::Acp,
            protocol_version,
            available,
            capabilities: ResolvedCapabilities::new(
                definition.declared_capabilities.clone(),
                driver_supported,
                verified,
            ),
            agent_info,
            diagnostics,
        })
    }

    async fn start(
        &self,
        ctx: DriverContext,
        definition: &AgentDefinition,
        config: AgentStartConfig,
    ) -> Result<DriverStartResult> {
        let cwd = validate_cwd(&config.cwd).await?;
        let sandbox_root = config
            .workspace_root
            .clone()
            .filter(|root| !root.trim().is_empty())
            .unwrap_or_else(|| cwd.clone());
        let process = process_config(definition, &config.env)?;
        let session = AcpSessionHandle::new(
            ctx.session.id.clone(),
            ctx.instance_id.clone(),
            ctx.emitter.clone(),
            self.config.permission_timeout,
            self.config.question_timeout,
        );
        *session.definition.lock() = Some(definition.clone());
        *session.workspace_root.lock() = sandbox_root;
        *session.session_cwd.lock() = cwd.clone();
        *session.launch_env.lock() = config.env.clone();
        let mcp_servers: Vec<Value> = config
            .extra
            .get("mcp_servers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        *session.mcp_servers.lock() = mcp_servers.clone();
        self.sessions
            .insert(ctx.session.id.clone(), Arc::clone(&session));

        let spawned = match self.start_connection(&process, &session).await {
            Ok(spawned) => spawned,
            Err(error) => {
                self.sessions.remove(&ctx.session.id);
                return Err(error);
            }
        };
        let native_session_id = match connection::new_native_session(
            &spawned.connection,
            &cwd,
            self.config.startup_timeout,
            &mcp_servers,
        )
        .await
        {
            Ok(native_session_id) => native_session_id,
            Err(error) => {
                self.sessions.remove(&ctx.session.id);
                let task = session.task.lock().take();
                if let Some(task) = task {
                    connection::abort(task).await;
                }
                return Err(error);
            }
        };
        *session.native_session_id.lock() = Some(native_session_id.clone());
        *session.protocol.lock() = Some(spawned.protocol);

        ctx.emitter.emit(
            AgentEventType::AgentRawEvent,
            json!({ "provider_session_id": native_session_id }),
        );
        ctx.emitter.emit(AgentEventType::SessionStarted, json!({}));
        common::emit_status(&ctx.emitter, SessionStatus::Running);

        let mut started = ctx.session;
        started.status = SessionStatus::Running;
        started.updated_at = fleet_core_common::now();
        let metadata = json!({
            "agent": {
                "definition_id": definition.id,
                "driver_id": "acp",
                "instance_id": ctx.instance_id,
            },
            "protocol": {
                "name": "acp",
                "version": spawned.protocol.number(),
            },
            "native": {
                "session_id": native_session_id.clone(),
            },
            "provider_session_id": native_session_id,
            "agent_info": spawned.agent_info,
            "capabilities": spawned.capabilities,
            "launch_snapshot": definition.snapshot()["launch_snapshot"].clone(),
        });
        Ok(DriverStartResult {
            session: started,
            process_id: None,
            metadata,
        })
    }

    async fn send_message(
        &self,
        instance: &AgentInstanceHandle,
        message: AgentPrompt,
    ) -> Result<PromptAcceptance> {
        let session = self.session(&instance.session_id)?;
        let connection = session.connection.lock().clone().ok_or_else(|| {
            RuntimeError::ProviderUnavailable("ACP connection is closed".to_string())
        })?;
        let native_session_id = session.native_session_id.lock().clone().ok_or_else(|| {
            RuntimeError::ProviderUnavailable("ACP session is not initialized".to_string())
        })?;

        if let (Some(fleet_message_id), Some(turn_id)) = (
            message
                .metadata
                .get("fleet_message_id")
                .and_then(Value::as_str),
            message.metadata.get("turn_id").and_then(Value::as_str),
        ) {
            if connection.protocol() == AcpProtocolVersion::V2 {
                session
                    .v2_state
                    .lock()
                    .begin_prompt(fleet_message_id.to_string(), Some(turn_id.to_string()));
            }
        }

        let native_message_id = connection::send_prompt(
            &connection,
            &session,
            &native_session_id,
            &message,
            self.config.startup_timeout,
        )
        .await?;
        if let Some(native_message_id) = &native_message_id {
            session
                .v2_state
                .lock()
                .accept_prompt(native_message_id.clone());
        }
        Ok(PromptAcceptance::now(native_message_id))
    }

    async fn interrupt(&self, instance: &AgentInstanceHandle) -> Result<()> {
        let session = self.session(&instance.session_id)?;
        let connection = session.connection.lock().clone().ok_or_else(|| {
            RuntimeError::ProviderUnavailable("ACP connection is closed".to_string())
        })?;
        let native_session_id = session.native_session_id.lock().clone().ok_or_else(|| {
            RuntimeError::ProviderUnavailable("ACP session is not initialized".to_string())
        })?;
        connection::cancel(&connection, &native_session_id).await
    }

    async fn resume(
        &self,
        instance: &AgentInstanceHandle,
        session: &AgentSession,
    ) -> Result<ResumeResult> {
        let handle = self.session(&instance.session_id)?;
        // Resume is check-then-act across a process spawn; serialize it so a
        // double click cannot create two agent processes for one session.
        let _resume_guard = handle.resume_lock.lock().await;
        let native_session_id = handle.native_session_id.lock().clone().ok_or_else(|| {
            RuntimeError::Unsupported(
                "the ACP session was not initialized and cannot be resumed".to_string(),
            )
        })?;
        let connection = handle.connection.lock().clone();
        let connection_alive = handle
            .task
            .lock()
            .as_ref()
            .is_some_and(|task| !task.is_finished());
        if let Some(_connection) = connection.as_ref().filter(|_| connection_alive) {
            // The process and connection are still alive, so the native
            // execution context was never lost.
            return Ok(ResumeResult {
                resumed: true,
                native_session_id: Some(native_session_id),
                protocol_version: handle.protocol.lock().map(|p| p.number()),
            });
        }
        if !handle.capabilities.lock().resume {
            return Err(RuntimeError::Unsupported(
                "the ACP agent does not advertise session/resume".to_string(),
            ));
        }
        let definition = handle.definition.lock().clone().ok_or_else(|| {
            RuntimeError::Unsupported(
                "the ACP launch definition is no longer available for resume".to_string(),
            )
        })?;
        let env = handle.launch_env.lock().clone();
        let process = process_config(&definition, &env)?;
        handle.terminals.clear();
        let spawned = self.start_connection(&process, &handle).await?;
        let cwd = {
            let stored = handle.session_cwd.lock().clone();
            if stored.is_empty() {
                session.cwd.clone()
            } else {
                stored
            }
        };
        let mcp_servers = handle.mcp_servers.lock().clone();
        // Drop mapper state from the crashed process: open partial messages
        // must not be completed later in the resumed turn.
        *handle.v1_state.lock() = Default::default();
        *handle.v2_state.lock() = Default::default();
        if let Err(error) = connection::resume_session(
            &spawned.connection,
            &native_session_id,
            &cwd,
            self.config.startup_timeout,
            &mcp_servers,
        )
        .await
        {
            // The fresh connection is unusable; tear it down so a later
            // resume or shutdown does not mistake it for a live process.
            let task = handle.task.lock().take();
            if let Some(task) = task {
                connection::abort(task).await;
            }
            *handle.connection.lock() = None;
            return Err(error);
        }
        Ok(ResumeResult {
            resumed: true,
            native_session_id: Some(native_session_id),
            protocol_version: Some(spawned.protocol.number()),
        })
    }

    async fn shutdown(&self, instance: &AgentInstanceHandle) -> Result<()> {
        let Some((_, session)) = self.sessions.remove(&instance.session_id) else {
            return Ok(());
        };
        session.begin_shutdown();
        session.terminals.clear();
        session
            .emitter
            .resolve_all_pending_questions(QuestionStatus::Cancelled);
        session
            .emitter
            .resolve_all_pending_permissions(PermissionStatus::Cancelled);
        let connection = session.connection.lock().clone();
        let native_session_id = session.native_session_id.lock().clone();
        if let (Some(connection), Some(native_session_id)) = (connection, native_session_id) {
            if let Err(error) = connection::close(
                &connection,
                &native_session_id,
                self.config.shutdown_timeout,
            )
            .await
            {
                tracing::debug!(error = %error, "ACP session close failed");
            }
        }
        let task = session.task.lock().take();
        if let Some(task) = task {
            connection::abort(task).await;
        }
        *session.connection.lock() = None;
        session
            .emitter
            .emit(AgentEventType::SessionTerminated, json!({}));
        common::emit_status(&session.emitter, SessionStatus::Terminated);
        Ok(())
    }

    async fn respond_permission(
        &self,
        _instance: &AgentInstanceHandle,
        _decision: PermissionDecision,
    ) -> Result<()> {
        // The driver's request handler owns the SDK responder; decisions are
        // delivered through the emitter's oneshot.
        Ok(())
    }

    async fn respond_question(
        &self,
        _instance: &AgentInstanceHandle,
        _decision: QuestionDecision,
    ) -> Result<()> {
        // The driver's request handler owns the SDK responder; decisions are
        // delivered through the emitter's oneshot.
        Ok(())
    }
}

impl AcpProcessConfig {
    fn clone_for_retry(&self) -> Self {
        Self {
            agent: self.agent.clone(),
            executable: self.executable.clone(),
            args: self.args.clone(),
        }
    }
}

async fn validate_cwd(raw: &str) -> Result<String> {
    if raw.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "ACP sessions require a working directory".to_string(),
        ));
    }
    let path: PathBuf = tokio::fs::canonicalize(raw)
        .await
        .map_err(|error| RuntimeError::InvalidRequest(format!("invalid cwd `{raw}`: {error}")))?;
    if !path.is_dir() {
        return Err(RuntimeError::InvalidRequest(format!(
            "cwd `{raw}` is not a directory"
        )));
    }
    Ok(path.to_string_lossy().to_string())
}

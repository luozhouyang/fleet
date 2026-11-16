//! ACP provider adapter: launches an external ACP agent over stdio.
//!
//! The adapter is responsible for the process, the ACP v1 handshake and the
//! per-session connection. Permission requests and form elicitations are
//! routed through the runtime-owned `AgentEventEmitter` oneshots, so
//! [`AgentProvider::respond_permission`] and
//! [`AgentProvider::respond_question`] are intentional no-ops.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::v1::schema::{
    ClientCapabilities, ElicitationCapabilities, ElicitationFormCapabilities, InitializeRequest,
    NewSessionRequest,
};
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::{
    on_receive_notification, on_receive_request, schema::ProtocolVersion, AcpAgent, AcpAgentConfig,
    Agent, Client,
};
use async_trait::async_trait;
use fleet_core_common::now;
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEventType, AgentSession, AgentStartConfig, PermissionDecision,
    ProviderInfo, ProviderKind, QuestionDecision, SessionStatus,
};
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::mapper;
use super::protocol::{
    CancelNotification, ContentBlock, CreateElicitationRequest, CreateElicitationResponse,
    ElicitationAction, PromptRequest, RequestPermissionRequest, SessionNotification, StopReason,
    TextContent,
};
use crate::error::{Result, RuntimeError};
use crate::event::AgentEventEmitter;
use crate::provider::{AgentProvider, ProviderContext};

struct AcpSessionState {
    /// Unified session id used by the runtime and the admin.
    session_id: String,
    /// Session id reported by the ACP agent.
    native_session_id: Mutex<Option<String>>,
    emitter: AgentEventEmitter,
    connection: Mutex<Option<ConnectionTo<Agent>>>,
    stream: Mutex<mapper::SessionUpdateState>,
    task: Mutex<Option<JoinHandle<()>>>,
}

/// Claude Code (ACP) provider.
///
/// A single provider instance owns the process for each unified session. ACP
/// agents are long-lived JSON-RPC peers; the provider keeps the connection
/// alive for the lifetime of the session.
pub struct AcpProvider {
    bin: String,
    args: Vec<String>,
    sessions: Mutex<HashMap<String, Arc<AcpSessionState>>>,
}

impl AcpProvider {
    /// Create a provider that launches `bin` with `args` over stdio.
    pub fn new(bin: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            bin: bin.into(),
            args,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Whether the configured ACP agent executable exists.
    pub fn available(bin: &str) -> bool {
        crate::provider::binary_exists(bin)
    }

    fn session(&self, session_id: &str) -> Result<Arc<AcpSessionState>> {
        self.sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))
    }

    async fn cleanup(&self, session_id: &str, state: &Arc<AcpSessionState>) {
        if let Some(task) = state.task.lock().take() {
            task.abort();
        }
        *state.connection.lock() = None;
        self.sessions.lock().remove(session_id);
    }
}

#[async_trait]
impl AgentProvider for AcpProvider {
    fn provider_id(&self) -> &'static str {
        "claude-acp"
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.provider_id().to_string(),
            kind: ProviderKind::Acp,
            display_name: "Claude Code (ACP)".to_string(),
            version: None,
            available: AcpProvider::available(&self.bin),
            capabilities: self.capabilities(),
            models: vec!["default".to_string()],
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            interrupt: true,
            resume: false,
            permission_approval: true,
            user_questions: true,
            structured_events: true,
            structured_diff: false,
            session_persistence: false,
            tool_calls: true,
            images: false,
        }
    }

    fn agent_type(&self) -> &'static str {
        "claude-acp"
    }

    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession> {
        let cwd = resolve_cwd(&config.cwd).await?;
        let session_id = ctx.session.id.clone();
        let state = Arc::new(AcpSessionState {
            session_id: session_id.clone(),
            native_session_id: Mutex::new(None),
            emitter: ctx.emitter.clone(),
            connection: Mutex::new(None),
            stream: Mutex::new(mapper::SessionUpdateState::default()),
            task: Mutex::new(None),
        });
        self.sessions
            .lock()
            .insert(session_id.clone(), Arc::clone(&state));

        let (cx_tx, cx_rx) = oneshot::channel();
        let state_permission = Arc::clone(&state);
        let state_question = Arc::clone(&state);
        let state_updates = Arc::clone(&state);
        let mut agent_config = AcpAgentConfig::new(&self.bin).args(self.args.clone());
        if !config.env.is_empty() {
            agent_config = agent_config.envs(config.env.clone());
        }
        let agent = AcpAgent::new(agent_config);

        let task = tokio::spawn(async move {
            let result = Client
                .v1()
                .name("agent-fleet")
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, _cx| {
                        let mut permission = mapper::map_permission_request(&request);
                        permission.session_id = state_permission.session_id.clone();
                        let pending = permission.clone();
                        let receiver = state_permission.emitter.request_permission(permission);
                        tokio::spawn(async move {
                            let decision = receiver.await.unwrap_or_else(|_| PermissionDecision {
                                permission_id: pending.id.clone(),
                                session_id: None,
                                approve: false,
                                option_id: None,
                                message: None,
                            });
                            let _ = responder
                                .respond(mapper::map_permission_response(&pending, &decision));
                        });
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_request(
                    async move |request: CreateElicitationRequest, responder, _cx| {
                        let mut question = match mapper::map_elicitation_request(&request) {
                            Ok(question) => question,
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    "declining unsupported ACP elicitation request"
                                );
                                let _ = responder.respond(CreateElicitationResponse::new(
                                    ElicitationAction::Decline,
                                ));
                                return Ok(());
                            }
                        };
                        question.session_id = state_question.session_id.clone();
                        let pending = question.clone();
                        let receiver = state_question.emitter.request_question(question);
                        tokio::spawn(async move {
                            let decision = receiver.await.unwrap_or_else(|_| QuestionDecision {
                                question_id: pending.id.clone(),
                                session_id: None,
                                answers: Vec::new(),
                                reject: true,
                                message: None,
                            });
                            let _ = responder
                                .respond(mapper::map_elicitation_response(&pending, &decision));
                        });
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        mapper::apply_session_update(
                            &state_updates.emitter,
                            &state_updates.stream,
                            &notification,
                        );
                        Ok(())
                    },
                    on_receive_notification!(),
                )
                .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
                    let _ = cx_tx.send(cx.clone());
                    cx.incoming_closed().await;
                    Ok(())
                })
                .await;
            if let Err(error) = result {
                tracing::warn!(error = %error, "ACP connection ended with an error");
            }
        });
        *state.task.lock() = Some(task);

        let cx = match cx_rx.await {
            Ok(cx) => cx,
            Err(_) => {
                self.cleanup(&session_id, &state).await;
                return Err(RuntimeError::ProviderUnavailable(format!(
                    "failed to start ACP agent `{}`",
                    self.bin
                )));
            }
        };
        *state.connection.lock() = Some(cx.clone());

        let initialize = InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
            ClientCapabilities::new().elicitation(
                ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()),
            ),
        );
        if let Err(error) = cx.send_request(initialize).block_task().await {
            self.cleanup(&session_id, &state).await;
            return Err(RuntimeError::ProviderUnavailable(format!(
                "ACP initialize failed: {error}"
            )));
        }

        let native_session_id = match cx
            .send_request(NewSessionRequest::new(cwd.clone()))
            .block_task()
            .await
        {
            Ok(response) => response.session_id.to_string(),
            Err(error) => {
                self.cleanup(&session_id, &state).await;
                return Err(RuntimeError::ProviderUnavailable(format!(
                    "ACP session/new failed: {error}"
                )));
            }
        };
        *state.native_session_id.lock() = Some(native_session_id.clone());

        ctx.emitter.emit(
            AgentEventType::AgentRawEvent,
            json!({ "provider_session_id": native_session_id }),
        );
        ctx.emitter.emit(AgentEventType::SessionStarted, json!({}));
        mapper::emit_status(&ctx.emitter, SessionStatus::Running);

        let mut session = ctx.session;
        session.status = SessionStatus::Running;
        session.updated_at = now();
        if !session.metadata.is_object() {
            session.metadata = json!({});
        }
        session.metadata["provider_session_id"] = json!(native_session_id);
        Ok(session)
    }

    async fn send_message(&self, session_id: &str, content: String) -> Result<()> {
        if content.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "message content must not be empty".to_string(),
            ));
        }
        let state = self.session(session_id)?;
        let connection =
            state.connection.lock().clone().ok_or_else(|| {
                RuntimeError::ProviderUnavailable("ACP connection is closed".into())
            })?;
        let native_session_id = state.native_session_id.lock().clone().ok_or_else(|| {
            RuntimeError::ProviderUnavailable("ACP session is not initialized".into())
        })?;
        let emitter = state.emitter.clone();
        let state_for_turn = Arc::clone(&state);

        tokio::spawn(async move {
            let prompt = PromptRequest::new(
                native_session_id,
                vec![ContentBlock::Text(TextContent::new(content))],
            );
            match connection.send_request(prompt).block_task().await {
                Ok(response) => {
                    mapper::complete_open_messages(&emitter, &state_for_turn.stream);
                    if matches!(response.stop_reason, StopReason::Cancelled) {
                        emitter.emit(
                            AgentEventType::TurnInterrupted,
                            json!({ "status": "interrupted" }),
                        );
                        mapper::emit_status(&emitter, SessionStatus::Interrupted);
                    } else {
                        emitter.emit(
                            AgentEventType::TurnCompleted,
                            json!({ "status": "completed" }),
                        );
                        mapper::emit_status(&emitter, SessionStatus::WaitingForInput);
                    }
                }
                Err(error) => {
                    emitter.emit(
                        AgentEventType::TurnFailed,
                        json!({ "error": error.to_string() }),
                    );
                    mapper::emit_status(&emitter, SessionStatus::Failed);
                }
            }
        });
        Ok(())
    }

    async fn interrupt(&self, session_id: &str) -> Result<()> {
        let state = self.session(session_id)?;
        let native_session_id = state.native_session_id.lock().clone().ok_or_else(|| {
            RuntimeError::ProviderUnavailable("ACP session is not initialized".into())
        })?;
        let connection =
            state.connection.lock().clone().ok_or_else(|| {
                RuntimeError::ProviderUnavailable("ACP connection is closed".into())
            })?;
        connection
            .send_notification(CancelNotification::new(native_session_id))
            .map_err(|error| {
                RuntimeError::ProviderUnavailable(format!("failed to cancel ACP turn: {error}"))
            })?;
        Ok(())
    }

    async fn resume(&self, _session_id: &str) -> Result<()> {
        Err(RuntimeError::Unsupported(
            "ACP sessions cannot be resumed; start a new session".to_string(),
        ))
    }

    async fn shutdown(&self, session_id: &str) -> Result<()> {
        let state = self
            .sessions
            .lock()
            .remove(session_id)
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))?;
        if let Some(task) = state.task.lock().take() {
            task.abort();
        }
        *state.connection.lock() = None;
        state
            .emitter
            .emit(AgentEventType::SessionTerminated, json!({}));
        mapper::emit_status(&state.emitter, SessionStatus::Terminated);
        Ok(())
    }

    async fn respond_permission(
        &self,
        _session_id: &str,
        _decision: PermissionDecision,
    ) -> Result<()> {
        // The runtime owns the oneshot registered by `request_permission`.
        Ok(())
    }

    async fn respond_question(&self, _session_id: &str, _decision: QuestionDecision) -> Result<()> {
        // The runtime owns the oneshot registered by `request_question`.
        Ok(())
    }
}

async fn resolve_cwd(raw: &str) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "ACP sessions require a working directory".to_string(),
        ));
    }
    let path = tokio::fs::canonicalize(raw)
        .await
        .map_err(|error| RuntimeError::InvalidRequest(format!("invalid cwd `{raw}`: {error}")))?;
    if !path.is_dir() {
        return Err(RuntimeError::InvalidRequest(format!(
            "cwd `{raw}` is not a directory"
        )));
    }
    Ok(path)
}

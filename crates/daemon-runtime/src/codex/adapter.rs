//! Codex app-server adapter (JSON-RPC 2.0 over stdio).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fleet_core_common::{new_id, now, ApiError};
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEventType, AgentPermissionRequest, AgentQuestion, AgentQuestionAnswer,
    AgentQuestionOption, AgentQuestionRequest, AgentSession, AgentStartConfig,
    MessageCompletedPayload, MessageDeltaPayload, MessageStartedPayload, PermissionDecision,
    PermissionStatus, ProviderInfo, ProviderKind, QuestionDecision, QuestionStatus,
    SessionErrorPayload, SessionStatus, StatusChangedPayload, ToolCallStatus, ToolEventPayload,
    ToolStatusPayload,
};
use fleet_daemon_process::{CommandSpec, ManagedProcess, ProcessEvent};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use super::mapper::{self, MapContext, MappedEvent};
use super::protocol::{
    self, id_key, parse_incoming, Incoming, InitializeResponse, RpcRequest, ThreadResponse,
    ToolRequestUserInputParams, TurnResponse,
};
use crate::error::{Result, RuntimeError};
use crate::event::AgentEventEmitter;
use crate::line_buffer::LineBuffer;
use crate::provider::{AgentProvider, ProviderContext};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(20);
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30);
const QUESTION_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
struct RpcFailure {
    code: i64,
    message: String,
}

type RpcOutcome = std::result::Result<Value, RpcFailure>;

struct CodexSessionState {
    process: Arc<ManagedProcess>,
    emitter: AgentEventEmitter,
    pending: Mutex<HashMap<String, oneshot::Sender<RpcOutcome>>>,
    next_id: AtomicU64,
    thread_id: Mutex<Option<String>>,
    native_turn_id: Mutex<Option<String>>,
    shutting_down: AtomicBool,
    reader: Mutex<Option<JoinHandle<()>>>,
    pending_permissions: Mutex<HashSet<String>>,
}

/// Codex app-server provider.
pub struct CodexProvider {
    bin: String,
    sessions: Mutex<HashMap<String, Arc<CodexSessionState>>>,
}

impl CodexProvider {
    /// Create a provider that launches `bin app-server --listen stdio://`.
    pub fn new(bin: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn command_spec(&self, config: &AgentStartConfig) -> CommandSpec {
        let mut spec = CommandSpec::new(&self.bin).args(["app-server", "--listen", "stdio://"]);
        if !config.cwd.is_empty() {
            spec = spec.cwd(config.cwd.clone());
        }
        for (key, value) in &config.env {
            spec = spec.env(key.clone(), value.clone());
        }
        spec
    }

    fn thread_start_params(config: &AgentStartConfig) -> Value {
        let mut params = serde_json::Map::new();
        if !config.cwd.is_empty() {
            params.insert("cwd".to_string(), json!(config.cwd));
        }
        if let Some(model) = &config.model {
            params.insert("model".to_string(), json!(model));
        }
        if let Some(prompt) = &config.system_prompt {
            params.insert("developerInstructions".to_string(), json!(prompt));
        }
        // Ask Codex to surface approval prompts rather than silently denying.
        params.insert("approvalPolicy".to_string(), json!("on-request"));
        Value::Object(params)
    }

    async fn request(
        state: &Arc<CodexSessionState>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = state.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (sender, receiver) = oneshot::channel();
        state.pending.lock().insert(id.to_string(), sender);
        let request = RpcRequest {
            id,
            method: method.to_string(),
            params,
        };
        let line = request.to_line()?;
        if let Err(error) = state.process.write_stdin(line.as_bytes()).await {
            state.pending.lock().remove(&id.to_string());
            return Err(RuntimeError::ProviderUnavailable(format!(
                "failed to write {method}: {error}"
            )));
        }

        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(failure))) => Err(RuntimeError::provider(
                failure.code.to_string(),
                failure.message,
            )),
            Ok(Err(_)) => Err(RuntimeError::provider(
                "rpc_closed",
                format!("{method} response channel closed"),
            )),
            Err(_) => {
                state.pending.lock().remove(&id.to_string());
                Err(RuntimeError::Timeout(format!(
                    "codex did not answer {method} in time"
                )))
            }
        }
    }
}

#[async_trait]
impl AgentProvider for CodexProvider {
    fn provider_id(&self) -> &'static str {
        "codex"
    }

    fn agent_type(&self) -> &'static str {
        "codex"
    }

    fn executable(&self) -> Option<&str> {
        Some(&self.bin)
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.provider_id().to_string(),
            kind: ProviderKind::Codex,
            display_name: "Codex".to_string(),
            version: None,
            available: crate::provider::binary_exists(&self.bin),
            capabilities: self.capabilities(),
            models: vec!["default".to_string()],
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
            // Diff notifications are not mapped by this adapter yet.
            structured_diff: false,
            session_persistence: true,
            tool_calls: true,
            images: false,
        }
    }

    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession> {
        let (process, events) = ManagedProcess::spawn_with_events(self.command_spec(&config))
            .await
            .map_err(|error| {
                RuntimeError::ProviderUnavailable(format!(
                    "failed to spawn codex app-server: {error}"
                ))
            })?;
        let process = Arc::new(process);

        let state = Arc::new(CodexSessionState {
            process,
            emitter: ctx.emitter.clone(),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            thread_id: Mutex::new(
                config
                    .extra
                    .get("provider_session_id")
                    .or_else(|| config.extra.get("resume"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ),
            native_turn_id: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            reader: Mutex::new(None),
            pending_permissions: Mutex::new(HashSet::new()),
        });

        let reader_state = Arc::clone(&state);
        let reader = tokio::spawn(async move {
            reader_loop(reader_state, events).await;
        });
        *state.reader.lock() = Some(reader);
        self.sessions
            .lock()
            .insert(ctx.session.id.clone(), Arc::clone(&state));

        let initialize = json!({
            "clientInfo": {
                "name": "agent-fleet",
                "title": "Agent Fleet",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {},
        });
        match Self::request(&state, "initialize", initialize, START_TIMEOUT).await {
            Ok(result) => {
                if let Ok(response) = serde_json::from_value::<InitializeResponse>(result) {
                    tracing::debug!(
                        user_agent = response.user_agent.as_deref().unwrap_or("unknown"),
                        platform = response.platform_os.as_deref().unwrap_or("unknown"),
                        "codex app-server initialized"
                    );
                }
            }
            Err(error) => {
                self.cleanup_failed_start(&ctx.session.id).await;
                return Err(error);
            }
        }

        let existing_thread = state.thread_id.lock().clone();
        let (method, params) = match existing_thread {
            Some(thread_id) => (
                "thread/resume",
                json!({"threadId": thread_id, "cwd": config.cwd, "model": config.model}),
            ),
            None => ("thread/start", Self::thread_start_params(&config)),
        };
        let thread = match Self::request(&state, method, params, START_TIMEOUT).await {
            Ok(result) => result,
            Err(error) => {
                self.cleanup_failed_start(&ctx.session.id).await;
                return Err(error);
            }
        };
        let thread_id = match serde_json::from_value::<ThreadResponse>(thread.clone()) {
            Ok(response) => response.thread.id,
            Err(error) => {
                self.cleanup_failed_start(&ctx.session.id).await;
                return Err(RuntimeError::provider(
                    "codex_protocol",
                    format!("{method} response was not understood: {error}"),
                ));
            }
        };
        *state.thread_id.lock() = Some(thread_id.clone());
        ctx.emitter.emit(
            AgentEventType::AgentRawEvent,
            json!({"provider_session_id": thread_id, "raw": thread}),
        );
        ctx.emitter.emit(AgentEventType::SessionStarted, json!({}));
        emit_status(&ctx.emitter, SessionStatus::Running);

        let mut session = ctx.session;
        session.status = SessionStatus::Running;
        session.metadata = json!({"provider_session_id": thread_id});
        session.updated_at = now();
        Ok(session)
    }

    async fn send_message(&self, session_id: &str, content: String) -> Result<()> {
        let state = self.session(session_id)?;
        let thread_id = state
            .thread_id
            .lock()
            .clone()
            .ok_or_else(|| RuntimeError::InvalidState("codex thread is not ready".into()))?;
        let params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": content}],
        });
        let result = CodexProvider::request(&state, "turn/start", params, REQUEST_TIMEOUT).await?;
        if let Ok(response) = serde_json::from_value::<TurnResponse>(result) {
            *state.native_turn_id.lock() = Some(response.turn.id);
        }
        Ok(())
    }

    async fn interrupt(&self, session_id: &str) -> Result<()> {
        let state = self.session(session_id)?;
        let thread_id = state.thread_id.lock().clone();
        let turn_id = state.native_turn_id.lock().clone();
        if let (Some(thread_id), Some(turn_id)) = (thread_id, turn_id) {
            let params = json!({"threadId": thread_id, "turnId": turn_id});
            if let Err(error) =
                CodexProvider::request(&state, "turn/interrupt", params, REQUEST_TIMEOUT).await
            {
                tracing::warn!(error = %error, "codex turn/interrupt failed");
            }
        }
        state.emitter.emit(
            AgentEventType::TurnInterrupted,
            json!({"reason": "interrupt"}),
        );
        emit_status(&state.emitter, SessionStatus::Interrupted);
        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        let state = self.session(session_id)?;
        let thread_id = state.thread_id.lock().clone().ok_or_else(|| {
            RuntimeError::Unsupported("no native codex thread id is available".to_string())
        })?;
        let params = json!({"threadId": thread_id});
        let result = CodexProvider::request(&state, "thread/resume", params, START_TIMEOUT).await?;
        if let Ok(response) = serde_json::from_value::<ThreadResponse>(result) {
            *state.thread_id.lock() = Some(response.thread.id);
        }
        state
            .emitter
            .emit(AgentEventType::SessionResumed, json!({}));
        emit_status(&state.emitter, SessionStatus::Running);
        Ok(())
    }

    async fn shutdown(&self, session_id: &str) -> Result<()> {
        let state = self
            .sessions
            .lock()
            .remove(session_id)
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))?;
        state.shutting_down.store(true, Ordering::SeqCst);
        if let Some(reader) = state.reader.lock().take() {
            reader.abort();
        }
        state.pending.lock().clear();
        let _ = state.process.terminate().await;
        state
            .emitter
            .emit(AgentEventType::SessionTerminated, json!({}));
        emit_status(&state.emitter, SessionStatus::Terminated);
        Ok(())
    }

    async fn respond_permission(
        &self,
        session_id: &str,
        decision: PermissionDecision,
    ) -> Result<()> {
        let state = self.session(session_id)?;
        let sender = {
            let mut permissions = state.emitter.permissions().lock();
            match permissions.get_mut(&decision.permission_id) {
                Some(entry) => entry.sender.take(),
                None => None,
            }
        };
        if let Some(sender) = sender {
            let _ = sender.send(decision);
            return Ok(());
        }
        if state
            .pending_permissions
            .lock()
            .remove(&decision.permission_id)
        {
            return Ok(());
        }
        Err(RuntimeError::PermissionNotFound(decision.permission_id))
    }
    async fn respond_question(&self, _session_id: &str, _decision: QuestionDecision) -> Result<()> {
        // The receiver registered in `handle_server_request` reads the decision
        // through the emitter's question sender; there is no second channel.
        Ok(())
    }
}

impl CodexProvider {
    fn session(&self, session_id: &str) -> Result<Arc<CodexSessionState>> {
        self.sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))
    }

    async fn cleanup_failed_start(&self, session_id: &str) {
        let state = self.sessions.lock().remove(session_id);
        if let Some(state) = state {
            state.shutting_down.store(true, Ordering::SeqCst);
            if let Some(reader) = state.reader.lock().take() {
                reader.abort();
            }
            let _ = state.process.terminate().await;
        }
    }
}

async fn reader_loop(state: Arc<CodexSessionState>, mut events: broadcast::Receiver<ProcessEvent>) {
    let mut buffer = LineBuffer::new();
    loop {
        match events.recv().await {
            Ok(ProcessEvent::Stdout(chunk)) => {
                for line in buffer.push(&chunk) {
                    handle_line(&state, &line);
                }
            }
            Ok(ProcessEvent::Stderr(chunk)) => {
                tracing::debug!(
                    session_id = %state.emitter.session_id(),
                    stderr = %String::from_utf8_lossy(&chunk),
                    "codex app-server stderr"
                );
            }
            Ok(ProcessEvent::Exited { code, success }) => {
                for line in buffer.finish() {
                    handle_line(&state, &line);
                }
                state
                    .emitter
                    .resolve_all_pending_questions(QuestionStatus::Cancelled);
                state
                    .emitter
                    .resolve_all_pending_permissions(PermissionStatus::Cancelled);
                if !state.shutting_down.load(Ordering::SeqCst) {
                    let error = ApiError::new(
                        "provider_exited",
                        format!(
                            "codex app-server exited unexpectedly (code: {code:?}, success: {success})"
                        ),
                    );
                    state.emitter.emit(
                        AgentEventType::SessionError,
                        serde_json::to_value(SessionErrorPayload { error }).unwrap_or_default(),
                    );
                    emit_status(&state.emitter, SessionStatus::Failed);
                }
                break;
            }
            Ok(ProcessEvent::Started { .. }) => {}
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "codex output receiver lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn handle_line(state: &Arc<CodexSessionState>, line: &str) {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(error = %error, "codex emitted a non-JSON line");
            state
                .emitter
                .emit(AgentEventType::AgentRawEvent, json!({"line": line}));
            return;
        }
    };

    match parse_incoming(&value) {
        Incoming::Response(response) => {
            let Some(key) = id_key(&response.id) else {
                return;
            };
            let pending = state.pending.lock().remove(&key);
            if let Some(sender) = pending {
                let outcome = match response.error {
                    Some(error) => Err(RpcFailure {
                        code: error.code,
                        message: error.message,
                    }),
                    None => Ok(response.result.unwrap_or(Value::Null)),
                };
                let _ = sender.send(outcome);
            }
        }
        Incoming::Notification(notification) => {
            handle_notification(state, &notification.method, notification.params);
        }
        Incoming::ServerRequest(request) => {
            let state = Arc::clone(state);
            tokio::spawn(async move {
                handle_server_request(state, request).await;
            });
        }
        Incoming::Unknown(value) => {
            state.emitter.emit(AgentEventType::AgentRawEvent, value);
        }
    }
}

fn handle_notification(state: &Arc<CodexSessionState>, method: &str, params: Value) {
    if method == "turn/started" {
        if let Some(turn_id) = params
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
        {
            *state.native_turn_id.lock() = Some(turn_id.to_string());
        }
    }

    let context = MapContext::new(state.emitter.session_id(), state.emitter.current_turn_id());
    let mapped = mapper::map_notification(method, &params, &context);
    for event in mapped {
        emit_mapped(state, event, method, &params);
    }
}

fn emit_mapped(state: &Arc<CodexSessionState>, event: MappedEvent, method: &str, params: &Value) {
    let emitter = &state.emitter;
    match event {
        MappedEvent::ThreadStarted { thread_id } => {
            *state.thread_id.lock() = Some(thread_id.clone());
            emitter.emit(
                AgentEventType::AgentRawEvent,
                json!({"provider_session_id": thread_id, "raw": params}),
            );
        }
        MappedEvent::TurnStarted => {
            emitter.emit(AgentEventType::TurnStarted, json!({"status": "running"}));
        }
        MappedEvent::TurnCompleted => {
            emitter.emit(
                AgentEventType::TurnCompleted,
                json!({"status": "completed"}),
            );
        }
        MappedEvent::TurnInterrupted => {
            emitter.emit(
                AgentEventType::TurnInterrupted,
                json!({"status": "interrupted"}),
            );
        }
        MappedEvent::TurnFailed { error } => {
            emitter.emit(AgentEventType::TurnFailed, json!({"error": error}));
            emitter.emit(
                AgentEventType::SessionError,
                serde_json::to_value(SessionErrorPayload { error }).unwrap_or_default(),
            );
        }
        MappedEvent::MessageStarted { message_id } => {
            emitter.emit_with(
                AgentEventType::MessageStarted,
                serde_json::to_value(MessageStartedPayload {
                    message_id: message_id.clone(),
                    role: fleet_core_protocol::agent::MessageRole::Assistant,
                })
                .unwrap_or_default(),
                None,
                Some(message_id),
                None,
            );
        }
        MappedEvent::MessageDelta { message_id, text } => {
            emitter.emit_with(
                AgentEventType::MessageDelta,
                serde_json::to_value(MessageDeltaPayload {
                    message_id: message_id.clone(),
                    text,
                    index: None,
                })
                .unwrap_or_default(),
                None,
                Some(message_id),
                None,
            );
        }
        MappedEvent::MessageCompleted { message } => {
            let message_id = message.id.clone();
            emitter.emit_with(
                AgentEventType::MessageCompleted,
                serde_json::to_value(MessageCompletedPayload {
                    message: message.clone(),
                })
                .unwrap_or_default(),
                message.turn_id,
                Some(message_id),
                None,
            );
        }
        MappedEvent::ToolStarted { tool } => {
            let tool_id = tool.id.clone();
            emitter.emit_with(
                AgentEventType::ToolStarted,
                serde_json::to_value(ToolEventPayload { tool }).unwrap_or_default(),
                None,
                None,
                Some(tool_id),
            );
        }
        MappedEvent::ToolCompleted { tool } => {
            let tool_id = tool.id.clone();
            emitter.emit_with(
                AgentEventType::ToolCompleted,
                serde_json::to_value(ToolEventPayload { tool }).unwrap_or_default(),
                None,
                None,
                Some(tool_id),
            );
        }
        MappedEvent::ToolFailed { tool, error } => {
            let tool_id = tool.id.clone();
            emitter.emit_with(
                AgentEventType::ToolFailed,
                serde_json::to_value(ToolStatusPayload {
                    tool_call_id: tool.id,
                    status: ToolCallStatus::Failed,
                    error: Some(error),
                })
                .unwrap_or_default(),
                None,
                None,
                Some(tool_id),
            );
        }
        MappedEvent::Status(status) => emit_status(emitter, status),
        MappedEvent::Raw(value) => {
            emitter.emit(
                AgentEventType::AgentRawEvent,
                json!({"method": method, "params": value}),
            );
        }
    }
}

async fn handle_server_request(state: Arc<CodexSessionState>, request: protocol::RpcServerRequest) {
    if request.method == "item/tool/requestUserInput" {
        handle_question_request(&state, &request).await;
        return;
    }

    let Some((tool_name, description, input, tool_call_id)) =
        approval_details(&request.method, &request.params)
    else {
        let response = json!({
            "id": request.id,
            "error": {"code": -32601, "message": format!("unsupported server request: {}", request.method)}
        });
        if let Ok(mut line) = serde_json::to_string(&response) {
            line.push('\n');
            let _ = state.process.write_stdin(line.as_bytes()).await;
        }
        state.emitter.emit(
            AgentEventType::AgentRawEvent,
            json!({"method": request.method, "params": request.params}),
        );
        return;
    };

    let permission = AgentPermissionRequest {
        id: new_id("permission"),
        session_id: state.emitter.session_id().to_string(),
        turn_id: request
            .params
            .get("turnId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| state.emitter.current_turn_id()),
        tool_call_id: Some(tool_call_id),
        tool_name,
        description,
        input,
        options: Vec::new(),
        status: PermissionStatus::Pending,
        created_at: now(),
        resolved_at: None,
        expires_at: None,
    };
    state
        .pending_permissions
        .lock()
        .insert(permission.id.clone());
    let receiver = state.emitter.request_permission(permission.clone());

    let decision = tokio::time::timeout(PERMISSION_TIMEOUT, receiver).await;
    let approve = matches!(decision, Ok(Ok(ref decision)) if decision.approve);
    if !matches!(decision, Ok(Ok(_))) {
        state
            .emitter
            .resolve_permission_locally(&permission.id, PermissionStatus::TimedOut);
    }
    state.pending_permissions.lock().remove(&permission.id);

    let result = approval_result(&request.method, approve);
    let response = json!({"id": request.id, "result": result});
    if let Ok(mut line) = serde_json::to_string(&response) {
        line.push('\n');
        if let Err(error) = state.process.write_stdin(line.as_bytes()).await {
            tracing::warn!(error = %error, "failed to send codex approval response");
        }
    }
}

/// Ask the user for input and answer Codex's `item/tool/requestUserInput`.
///
/// The response is always written so the app-server never blocks: an empty
/// answer list is returned when the user rejects the question, the session
/// shuts down, or the 600s timeout elapses.
async fn handle_question_request(
    state: &Arc<CodexSessionState>,
    request: &protocol::RpcServerRequest,
) {
    let Some(mut question) = question_details(&request.method, &request.params) else {
        write_server_response(
            state,
            json!({
                "id": request.id,
                "error": {"code": -32602, "message": "invalid requestUserInput params"}
            }),
        )
        .await;
        state.emitter.emit(
            AgentEventType::AgentRawEvent,
            json!({"method": request.method, "params": request.params}),
        );
        return;
    };
    question.session_id = state.emitter.session_id().to_string();

    let receiver = state.emitter.request_question(question.clone());
    let decision = tokio::time::timeout(QUESTION_TIMEOUT, receiver).await;
    let answers = match decision {
        Ok(Ok(decision)) if !decision.reject => decision.answers,
        Ok(Ok(_)) => Vec::new(),
        Ok(Err(_)) => {
            state
                .emitter
                .resolve_question_locally(&question.id, QuestionStatus::Cancelled);
            Vec::new()
        }
        Err(_) => {
            state
                .emitter
                .resolve_question_locally(&question.id, QuestionStatus::TimedOut);
            Vec::new()
        }
    };

    if !write_server_response(
        state,
        json!({"id": request.id, "result": question_result(&answers)}),
    )
    .await
    {
        state.emitter.fail_question_delivery(&question.id);
    }
}

async fn write_server_response(state: &Arc<CodexSessionState>, response: Value) -> bool {
    let Ok(mut line) = serde_json::to_string(&response) else {
        return false;
    };
    line.push('\n');
    match state.process.write_stdin(line.as_bytes()).await {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(error = %error, "failed to send codex server response");
            false
        }
    }
}

fn question_details(method: &str, params: &Value) -> Option<AgentQuestionRequest> {
    if method != "item/tool/requestUserInput" {
        return None;
    }
    let params: ToolRequestUserInputParams = serde_json::from_value(params.clone()).ok()?;
    Some(AgentQuestionRequest {
        id: new_id("question"),
        session_id: String::new(), // filled by the emitter
        turn_id: Some(params.turn_id),
        tool_call_id: Some(params.item_id),
        questions: params
            .questions
            .into_iter()
            .map(|question| AgentQuestion {
                id: question.id,
                header: Some(question.header),
                question: question.question,
                options: question
                    .options
                    .unwrap_or_default()
                    .into_iter()
                    .map(|option| AgentQuestionOption {
                        label: option.label,
                        description: Some(option.description),
                        value: None,
                    })
                    .collect(),
                multi_select: false,
                allow_custom: question.is_other,
                is_secret: question.is_secret,
            })
            .collect(),
        answers: Vec::new(),
        status: QuestionStatus::Pending,
        created_at: now(),
        resolved_at: None,
        expires_at: None,
    })
}

fn question_result(answers: &[AgentQuestionAnswer]) -> Value {
    let mut by_id = serde_json::Map::new();
    for answer in answers {
        let mut selected = answer.selected.clone();
        if let Some(custom) = answer.custom.as_deref().map(str::trim) {
            if !custom.is_empty() && !selected.iter().any(|item| item == custom) {
                selected.push(custom.to_string());
            }
        }
        by_id.insert(answer.id.clone(), json!({"answers": selected}));
    }
    json!({"answers": by_id})
}

fn approval_details(method: &str, params: &Value) -> Option<(String, String, Value, String)> {
    match method {
        "item/commandExecution/requestApproval" => {
            let command = params
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("command");
            let item_id = params
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some((
                "command_execution".to_string(),
                format!("Allow Codex to run `{command}`?"),
                params.clone(),
                item_id,
            ))
        }
        "item/fileChange/requestApproval" => {
            let item_id = params
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some((
                "file_change".to_string(),
                "Allow Codex to apply file changes?".to_string(),
                params.clone(),
                item_id,
            ))
        }
        "execCommandApproval" => {
            let command = params
                .get("command")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_else(|| "command".to_string());
            let call_id = params
                .get("callId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some((
                "command".to_string(),
                format!("Allow Codex to run `{command}`?"),
                params.clone(),
                call_id,
            ))
        }
        "applyPatchApproval" => {
            let call_id = params
                .get("callId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some((
                "apply_patch".to_string(),
                "Allow Codex to apply a patch?".to_string(),
                params.clone(),
                call_id,
            ))
        }
        _ => None,
    }
}

fn approval_result(method: &str, approve: bool) -> Value {
    match method {
        "execCommandApproval" | "applyPatchApproval" => {
            json!({"decision": if approve { "approved" } else { "denied" }})
        }
        _ => json!({"decision": if approve { "accept" } else { "decline" }}),
    }
}

fn emit_status(emitter: &AgentEventEmitter, status: SessionStatus) {
    emitter.emit(
        AgentEventType::AgentStatusChanged,
        serde_json::to_value(StatusChangedPayload {
            status,
            reason: None,
        })
        .unwrap_or_default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_response_shapes_match_schema() {
        assert_eq!(
            approval_result("item/commandExecution/requestApproval", true)["decision"],
            "accept"
        );
        assert_eq!(
            approval_result("item/commandExecution/requestApproval", false)["decision"],
            "decline"
        );
        assert_eq!(
            approval_result("execCommandApproval", true)["decision"],
            "approved"
        );
        assert_eq!(
            approval_result("applyPatchApproval", false)["decision"],
            "denied"
        );
    }

    #[test]
    fn request_user_input_fixture_is_recognized() {
        let fixture = include_str!("fixtures/request_user_input.jsonl");
        let value: Value = serde_json::from_str(fixture.trim()).unwrap();
        let Incoming::ServerRequest(request) = parse_incoming(&value) else {
            panic!("expected server request");
        };
        let mapped = question_details(&request.method, &request.params).expect("question mapping");
        assert_eq!(mapped.questions.len(), 1);
        assert_eq!(mapped.questions[0].id, "target");
        assert_eq!(mapped.questions[0].header.as_deref(), Some("Deploy"));
        assert_eq!(mapped.questions[0].question, "Which environment?");
        assert_eq!(mapped.questions[0].options.len(), 2);
        assert_eq!(
            mapped.questions[0].options[0].description.as_deref(),
            Some("Safe")
        );
        assert!(mapped.questions[0].allow_custom);
        assert!(!mapped.questions[0].multi_select);
        assert_eq!(mapped.turn_id.as_deref(), Some("turn_1"));
        assert_eq!(mapped.tool_call_id.as_deref(), Some("item_q"));
    }

    #[test]
    fn question_response_shape_matches_schema() {
        let answers = vec![AgentQuestionAnswer {
            id: "target".to_string(),
            selected: vec!["staging".to_string()],
            custom: None,
        }];
        assert_eq!(
            question_result(&answers),
            json!({"answers": {"target": {"answers": ["staging"]}}})
        );
        assert_eq!(question_result(&[]), json!({"answers": {}}));

        let custom = vec![AgentQuestionAnswer {
            id: "target".to_string(),
            selected: Vec::new(),
            custom: Some("  eu-west-1 ".to_string()),
        }];
        assert_eq!(
            question_result(&custom),
            json!({"answers": {"target": {"answers": ["eu-west-1"]}}})
        );
    }

    #[test]
    fn only_request_user_input_maps_to_questions() {
        assert!(question_details("item/commandExecution/requestApproval", &json!({})).is_none());
    }

    #[test]
    fn approval_fixture_is_recognized() {
        let fixture = include_str!("fixtures/approval.jsonl");
        for line in fixture.lines() {
            let value: Value = serde_json::from_str(line).expect("fixture line");
            let Incoming::ServerRequest(request) = parse_incoming(&value) else {
                panic!("expected a server request");
            };
            let details = approval_details(&request.method, &request.params);
            assert!(
                details.is_some(),
                "missing approval mapping for {}",
                request.method
            );
        }
    }
}

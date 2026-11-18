//! Claude Code CLI adapter (`claude -p --input-format stream-json ...`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use fleet_core_common::{new_id, now};
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEventType, AgentMessage, AgentSession, AgentStartConfig,
    MessageCompletedPayload, MessageDeltaPayload, MessageRole, MessageStartedPayload,
    PermissionDecision, ProviderInfo, ProviderKind, SessionErrorPayload, SessionStatus,
    StatusChangedPayload, ToolCallStatus, ToolEventPayload, ToolStatusPayload,
};
use fleet_daemon_process::{CommandSpec, ManagedProcess, ProcessEvent};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::mapper::{self, MapContext, MappedEvent};
use super::protocol::ClaudeLine;
use crate::error::{Result, RuntimeError};
use crate::event::AgentEventEmitter;
use crate::line_buffer::LineBuffer;
use crate::provider::{AgentProvider, ProviderContext};

struct StreamingMessage {
    id: String,
    content: String,
}

struct ClaudeSessionState {
    process: RwLock<Option<Arc<ManagedProcess>>>,
    emitter: AgentEventEmitter,
    native_session_id: Mutex<Option<String>>,
    config: AgentStartConfig,
    shutting_down: AtomicBool,
    stream: Mutex<Option<StreamingMessage>>,
    /// Native message id announced by `message_start` for the active stream.
    pending_stream_id: Mutex<Option<String>>,
    /// Text of the most recently finished stream. Claude emits both
    /// `message_stop` and a final `assistant` line for the same message.
    last_streamed_text: Mutex<Option<String>>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

/// Claude Code CLI provider.
pub struct ClaudeProvider {
    bin: String,
    resume_supported: bool,
    sessions: Mutex<HashMap<String, Arc<ClaudeSessionState>>>,
}

impl ClaudeProvider {
    /// Create a provider using `bin`.
    ///
    /// Resume is advertised when the binary supports `--resume`; the default is
    /// `true` because Claude Code 2.1.x supports it. Use
    /// [`ClaudeProvider::with_resume_support`] for older or wrapped binaries.
    pub fn new(bin: impl Into<String>) -> Self {
        Self::with_resume_support(bin, true)
    }

    /// Create a provider with an explicit `--resume` capability.
    pub fn with_resume_support(bin: impl Into<String>, resume_supported: bool) -> Self {
        Self {
            bin: bin.into(),
            resume_supported,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Whether this provider advertises `--resume`.
    pub fn resume_supported(&self) -> bool {
        self.resume_supported
    }

    fn command_spec(&self, config: &AgentStartConfig, resume_id: Option<&str>) -> CommandSpec {
        let mut spec = CommandSpec::new(&self.bin).args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--verbose",
        ]);
        if let Some(model) = &config.model {
            spec = spec.args(["--model", model]);
        }
        if let Some(prompt) = &config.system_prompt {
            spec = spec.args(["--append-system-prompt", prompt]);
        }
        if !config.allowed_tools.is_empty() {
            spec = spec.args(["--allowedTools", &config.allowed_tools.join(",")]);
        }
        if let Some(add_dirs) = config
            .extra
            .get("add_dirs")
            .and_then(serde_json::Value::as_array)
        {
            for dir in add_dirs.iter().filter_map(serde_json::Value::as_str) {
                spec = spec.args(["--add-dir", dir]);
            }
        }
        if let Some(resume_id) = resume_id {
            spec = spec.args(["--resume", resume_id]);
        }
        if !config.cwd.is_empty() {
            spec = spec.cwd(config.cwd.clone());
        }
        for (key, value) in &config.env {
            spec = spec.env(key.clone(), value.clone());
        }
        spec
    }
}

#[async_trait]
impl AgentProvider for ClaudeProvider {
    fn provider_id(&self) -> &'static str {
        "claude"
    }

    fn agent_type(&self) -> &'static str {
        "claude-code"
    }

    fn executable(&self) -> Option<&str> {
        Some(&self.bin)
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.provider_id().to_string(),
            kind: ProviderKind::Claude,
            display_name: "Claude Code".to_string(),
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
            resume: self.resume_supported,
            // Claude's stream-json CLI mode does not expose a documented
            // permission-response channel in this adapter.
            permission_approval: false,
            user_questions: false,
            structured_events: true,
            structured_diff: false,
            session_persistence: true,
            tool_calls: true,
            images: false,
        }
    }

    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession> {
        let resume_id = config
            .extra
            .get("provider_session_id")
            .or_else(|| config.extra.get("resume"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if resume_id.is_some() && !self.resume_supported {
            return Err(RuntimeError::Unsupported(
                "this Claude binary does not support --resume".to_string(),
            ));
        }

        let spec = self.command_spec(&config, resume_id.as_deref());
        let (process, events) = ManagedProcess::spawn_with_events(spec)
            .await
            .map_err(|error| {
                RuntimeError::ProviderUnavailable(format!("failed to spawn claude: {error}"))
            })?;
        let process = Arc::new(process);

        let state = Arc::new(ClaudeSessionState {
            process: RwLock::new(Some(Arc::clone(&process))),
            emitter: ctx.emitter.clone(),
            native_session_id: Mutex::new(resume_id),
            config: config.clone(),
            shutting_down: AtomicBool::new(false),
            stream: Mutex::new(None),
            pending_stream_id: Mutex::new(None),
            last_streamed_text: Mutex::new(None),
            reader: Mutex::new(None),
        });

        let reader_state = Arc::clone(&state);
        let reader = tokio::spawn(async move {
            reader_loop(reader_state, events).await;
        });
        *state.reader.lock() = Some(reader);

        self.sessions
            .lock()
            .insert(ctx.session.id.clone(), Arc::clone(&state));

        ctx.emitter.emit(AgentEventType::SessionStarted, json!({}));
        emit_status(&ctx.emitter, SessionStatus::Running);

        let mut session = ctx.session;
        session.status = SessionStatus::Running;
        session.updated_at = now();
        Ok(session)
    }

    async fn send_message(&self, session_id: &str, content: String) -> Result<()> {
        let state = self.session(session_id)?;
        let process =
            state.process.read().clone().ok_or_else(|| {
                RuntimeError::InvalidState("claude process is not running".into())
            })?;
        let line = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": content}]
            }
        });
        let mut bytes = serde_json::to_vec(&line)?;
        bytes.push(b'\n');
        process.write_stdin(&bytes).await?;
        Ok(())
    }

    async fn interrupt(&self, session_id: &str) -> Result<()> {
        let state = self.session(session_id)?;
        let process = state.process.read().clone();
        if let Some(process) = process {
            process.interrupt().await?;
        }
        let _ = finish_stream(&state);
        state
            .emitter
            .emit(AgentEventType::TurnInterrupted, json!({"reason": "sigint"}));
        emit_status(&state.emitter, SessionStatus::Interrupted);
        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        if !self.resume_supported {
            return Err(RuntimeError::Unsupported(
                "this Claude binary does not support --resume".to_string(),
            ));
        }
        let state = self.session(session_id)?;
        let native_id = state.native_session_id.lock().clone().ok_or_else(|| {
            RuntimeError::Unsupported(
                "no native Claude session id is available to resume".to_string(),
            )
        })?;

        if let Some(reader) = state.reader.lock().take() {
            reader.abort();
        }
        let previous = state.process.write().take();
        if let Some(process) = previous {
            let _ = process.terminate().await;
        }
        state.shutting_down.store(false, Ordering::SeqCst);

        let spec = self.command_spec(&state.config, Some(native_id.as_str()));
        let (process, events) = ManagedProcess::spawn_with_events(spec)
            .await
            .map_err(|error| {
                RuntimeError::ProviderUnavailable(format!("failed to resume claude: {error}"))
            })?;
        let process = Arc::new(process);
        *state.process.write() = Some(process);

        let reader_state = Arc::clone(&state);
        let reader = tokio::spawn(async move {
            reader_loop(reader_state, events).await;
        });
        *state.reader.lock() = Some(reader);

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
        let process = state.process.write().take();
        if let Some(process) = process {
            let _ = process.terminate().await;
        }
        state
            .emitter
            .emit(AgentEventType::SessionTerminated, json!({}));
        emit_status(&state.emitter, SessionStatus::Terminated);
        Ok(())
    }

    async fn respond_permission(
        &self,
        _session_id: &str,
        _decision: PermissionDecision,
    ) -> Result<()> {
        Err(RuntimeError::Unsupported(
            "the Claude CLI adapter does not implement permission approval".to_string(),
        ))
    }
    async fn respond_question(
        &self,
        _session_id: &str,
        _decision: fleet_core_protocol::agent::QuestionDecision,
    ) -> Result<()> {
        Err(RuntimeError::Unsupported(
            "the Claude CLI adapter does not implement user questions".to_string(),
        ))
    }
}

impl ClaudeProvider {
    fn session(&self, session_id: &str) -> Result<Arc<ClaudeSessionState>> {
        self.sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))
    }
}

async fn reader_loop(
    state: Arc<ClaudeSessionState>,
    mut events: broadcast::Receiver<ProcessEvent>,
) {
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
                    "claude stderr"
                );
            }
            Ok(ProcessEvent::Exited { code, success }) => {
                for line in buffer.finish() {
                    handle_line(&state, &line);
                }
                if !state.shutting_down.load(Ordering::SeqCst) {
                    let error = fleet_core_common::ApiError::new(
                        "provider_exited",
                        format!("claude exited unexpectedly (code: {code:?}, success: {success})"),
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
                tracing::warn!(skipped, "claude output receiver lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn handle_line(state: &ClaudeSessionState, line: &str) {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(error = %error, "claude emitted a non-JSON line");
            state.emitter.emit(
                AgentEventType::AgentRawEvent,
                json!({"line": line, "parse_error": error.to_string()}),
            );
            return;
        }
    };
    let parsed: ClaudeLine = match serde_json::from_value(value.clone()) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::debug!(error = %error, "claude line did not match the protocol");
            emit_raw(state, value);
            return;
        }
    };
    if let Some(session_id) = parsed.session_id().map(str::to_string) {
        let mut native = state.native_session_id.lock();
        if native.as_deref() != Some(session_id.as_str()) {
            *native = Some(session_id);
        }
    }

    let streamed_text = if matches!(parsed, ClaudeLine::Assistant { .. }) {
        let streamed = finish_stream(state);
        let previous = state.last_streamed_text.lock().take();
        streamed.or(previous)
    } else {
        None
    };

    let context = MapContext::new(state.emitter.session_id(), state.emitter.current_turn_id());
    let mut mapped = mapper::map_line(&parsed, &context);
    if let Some(streamed) = streamed_text {
        suppress_duplicate_assistant_text(&mut mapped, &streamed);
    }
    if mapped.is_empty() {
        if matches!(parsed, ClaudeLine::Unknown) {
            emit_raw(state, value);
        }
        return;
    }
    for event in mapped {
        emit_mapped(state, event, &value);
    }
}

fn emit_mapped(state: &ClaudeSessionState, event: MappedEvent, raw: &Value) {
    let emitter = &state.emitter;
    match event {
        MappedEvent::SessionInit { session_id } => {
            *state.native_session_id.lock() = Some(session_id.clone());
            emitter.emit(
                AgentEventType::AgentRawEvent,
                json!({
                    "provider_session_id": session_id,
                    "raw": raw,
                }),
            );
        }
        MappedEvent::MessageStarted { message_id, role } => {
            emitter.emit_with(
                AgentEventType::MessageStarted,
                serde_json::to_value(MessageStartedPayload {
                    message_id: message_id.clone(),
                    role,
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
        MappedEvent::TurnCompleted => {
            emitter.emit(
                AgentEventType::TurnCompleted,
                json!({"status": "completed"}),
            );
        }
        MappedEvent::TurnFailed { error } => {
            emitter.emit(AgentEventType::TurnFailed, json!({"error": error}));
            emitter.emit(
                AgentEventType::SessionError,
                serde_json::to_value(SessionErrorPayload { error }).unwrap_or_default(),
            );
        }
        MappedEvent::Status(status) => emit_status(emitter, status),
        MappedEvent::StreamTextDelta(text) => stream_delta(state, &text),
        MappedEvent::StreamMessageStart { message_id } => {
            *state.pending_stream_id.lock() = Some(message_id);
        }
        MappedEvent::StreamMessageStop => {
            finish_stream(state);
        }
        MappedEvent::Raw(value) => emit_raw(state, value),
    }
}

fn stream_delta(state: &ClaudeSessionState, text: &str) {
    let emitter = &state.emitter;
    let mut stream = state.stream.lock();
    if stream.is_none() {
        let id = state
            .pending_stream_id
            .lock()
            .take()
            .unwrap_or_else(|| new_id("message"));
        *stream = Some(StreamingMessage {
            id: id.clone(),
            content: String::new(),
        });
        emitter.emit_with(
            AgentEventType::MessageStarted,
            serde_json::to_value(MessageStartedPayload {
                message_id: id.clone(),
                role: MessageRole::Assistant,
            })
            .unwrap_or_default(),
            None,
            Some(id),
            None,
        );
    }
    let Some(stream) = stream.as_mut() else {
        return;
    };
    stream.content.push_str(text);
    let id = stream.id.clone();
    emitter.emit_with(
        AgentEventType::MessageDelta,
        serde_json::to_value(MessageDeltaPayload {
            message_id: id.clone(),
            text: text.to_string(),
            index: None,
        })
        .unwrap_or_default(),
        None,
        Some(id),
        None,
    );
}

fn finish_stream(state: &ClaudeSessionState) -> Option<String> {
    let stream = state.stream.lock().take();
    let Some(stream) = stream else {
        *state.pending_stream_id.lock() = None;
        return None;
    };
    let content = stream.content.clone();
    *state.pending_stream_id.lock() = None;
    *state.last_streamed_text.lock() = Some(content.clone());
    let message = AgentMessage {
        id: stream.id.clone(),
        session_id: state.emitter.session_id().to_string(),
        turn_id: state.emitter.current_turn_id(),
        role: MessageRole::Assistant,
        content,
        created_at: now(),
    };
    state.emitter.emit_with(
        AgentEventType::MessageCompleted,
        serde_json::to_value(MessageCompletedPayload { message }).unwrap_or_default(),
        None,
        Some(stream.id),
        None,
    );
    Some(stream.content)
}

/// Claude emits a final `assistant` line after partial streaming. When its text
/// matches the already-streamed message, drop the duplicate message events but
/// keep tool events.
fn suppress_duplicate_assistant_text(events: &mut Vec<MappedEvent>, streamed: &str) {
    let assistant_text = events.iter().find_map(|event| match event {
        MappedEvent::MessageDelta { text, .. } => Some(text.as_str()),
        MappedEvent::MessageCompleted { message } => Some(message.content.as_str()),
        _ => None,
    });
    if assistant_text == Some(streamed) {
        events.retain(|event| {
            !matches!(
                event,
                MappedEvent::MessageStarted { .. }
                    | MappedEvent::MessageDelta { .. }
                    | MappedEvent::MessageCompleted { .. }
            )
        });
    }
}

fn emit_raw(state: &ClaudeSessionState, value: Value) {
    state.emitter.emit(AgentEventType::AgentRawEvent, value);
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
    fn suppresses_duplicate_final_assistant_text() {
        let message = AgentMessage {
            id: "msg_1".into(),
            session_id: "session_1".into(),
            turn_id: Some("turn_1".into()),
            role: MessageRole::Assistant,
            content: "hello".into(),
            created_at: now(),
        };
        let mut events = vec![
            MappedEvent::MessageStarted {
                message_id: "msg_1".into(),
                role: MessageRole::Assistant,
            },
            MappedEvent::MessageDelta {
                message_id: "msg_1".into(),
                text: "hello".into(),
            },
            MappedEvent::MessageCompleted { message },
            MappedEvent::Status(SessionStatus::Running),
        ];
        suppress_duplicate_assistant_text(&mut events, "hello");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], MappedEvent::Status(_)));
    }

    #[test]
    fn requests_partial_message_streaming() {
        let provider = ClaudeProvider::new("claude");
        let config = AgentStartConfig {
            cwd: ".".to_string(),
            ..AgentStartConfig::default()
        };
        let spec = provider.command_spec(&config, None);
        assert!(
            spec.args
                .iter()
                .any(|arg| arg == "--include-partial-messages"),
            "Claude must receive --include-partial-messages so stream_event deltas are emitted"
        );
        assert!(spec.args.iter().any(|arg| arg == "stream-json"));
    }
}

//! Fully functional in-process mock provider used by tests and E2E flows.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fleet_core_common::{error_codes, new_id, now, ApiError};
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEventType, AgentMessage, AgentPermissionRequest, AgentSession,
    AgentStartConfig, AgentToolCall, MessageCompletedPayload, MessageDeltaPayload, MessageRole,
    MessageStartedPayload, PermissionDecision, PermissionStatus, ProviderInfo, ProviderKind,
    SessionErrorPayload, SessionStatus, StatusChangedPayload, ToolCallStatus, ToolEventPayload,
    ToolStatusPayload,
};
use parking_lot::Mutex;
use serde_json::json;

use crate::cancel::CancelFlag;
use crate::error::{Result, RuntimeError};
use crate::event::AgentEventEmitter;
use crate::provider::{AgentProvider, ProviderContext};

const DEFAULT_DELAY_MS: u64 = 5;
const SLOW_DELAY_MS: u64 = 100;
const SLOW_WORDS: usize = 50;
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct ActiveTurn {
    turn_id: String,
    cancel: CancelFlag,
}

struct MockSessionState {
    delay: Duration,
    emitter: AgentEventEmitter,
    active: Mutex<Option<ActiveTurn>>,
    pending_permissions: Mutex<HashSet<String>>,
}

impl MockSessionState {
    fn current_turn_id(&self) -> Option<String> {
        self.emitter.current_turn_id()
    }

    fn start_turn(&self) -> (String, CancelFlag) {
        let turn_id = self.current_turn_id().unwrap_or_else(|| new_id("turn"));
        self.emitter.set_current_turn(Some(turn_id.clone()));
        let cancel = CancelFlag::new();
        if let Some(previous) = self.active.lock().replace(ActiveTurn {
            turn_id: turn_id.clone(),
            cancel: cancel.clone(),
        }) {
            previous.cancel.cancel();
        }
        (turn_id, cancel)
    }

    fn finish_turn(&self, turn_id: &str) {
        let mut active = self.active.lock();
        if active.as_ref().is_some_and(|turn| turn.turn_id == turn_id) {
            *active = None;
        }
    }
}

/// In-process provider with deterministic streaming behaviour.
///
/// Message content selects the scenario:
///
/// | content contains | behaviour |
/// |---|---|
/// | `/tool` | request permission for a `Bash` tool call |
/// | `/fail` | fail the turn with `mock_failure` |
/// | `/slow` | stream 50 words with 100 ms delays |
/// | `/never` | wait until interrupted or shut down |
/// | anything else | stream `Echo: <text>` word by word |
#[derive(Default)]
pub struct MockProvider {
    sessions: Mutex<HashMap<String, Arc<MockSessionState>>>,
}

impl MockProvider {
    /// Create a new mock provider with no sessions.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AgentProvider for MockProvider {
    fn provider_id(&self) -> &'static str {
        "mock"
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.provider_id().to_string(),
            kind: ProviderKind::Mock,
            display_name: "Mock Agent".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            available: true,
            capabilities: self.capabilities(),
            models: vec!["mock-1".to_string()],
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            interrupt: true,
            resume: true,
            permission_approval: true,
            structured_events: true,
            structured_diff: false,
            session_persistence: true,
            tool_calls: true,
            images: false,
        }
    }

    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession> {
        let delay_ms = config
            .extra
            .get("delay_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_DELAY_MS)
            .min(5_000);
        let state = Arc::new(MockSessionState {
            delay: Duration::from_millis(delay_ms),
            emitter: ctx.emitter.clone(),
            active: Mutex::new(None),
            pending_permissions: Mutex::new(HashSet::new()),
        });
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
        let state = self
            .sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))?;
        let emitter = state.emitter.clone();
        let (turn_id, cancel) = state.start_turn();
        let state_for_task = Arc::clone(&state);

        tokio::spawn(async move {
            run_turn(state_for_task, emitter, content, turn_id, cancel).await;
        });
        Ok(())
    }

    async fn interrupt(&self, session_id: &str) -> Result<()> {
        let state = self
            .sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))?;
        let emitter = state.emitter.clone();
        let active = state.active.lock().as_ref().cloned();
        if let Some(active) = active {
            active.cancel.cancel();
            emitter.emit(
                AgentEventType::TurnInterrupted,
                json!({"reason": "interrupted by user"}),
            );
            emit_status(&emitter, SessionStatus::Interrupted);
        }
        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        let state = self
            .sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))?;
        let emitter = state.emitter.clone();
        emitter.emit(AgentEventType::SessionResumed, json!({}));
        emit_status(&emitter, SessionStatus::Running);
        Ok(())
    }

    async fn shutdown(&self, session_id: &str) -> Result<()> {
        let state = self.sessions.lock().remove(session_id);
        let Some(state) = state else {
            return Err(RuntimeError::SessionNotFound(session_id.to_string()));
        };
        if let Some(active) = state.active.lock().take() {
            active.cancel.cancel();
        }
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
        let state = self
            .sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))?;
        // The runtime resolves the shared oneshot. This method keeps the
        // provider usable when called directly: it validates the permission
        // and, when the runtime has not already taken the sender, resolves it.
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
            .contains(&decision.permission_id)
        {
            return Ok(());
        }
        Err(RuntimeError::PermissionNotFound(decision.permission_id))
    }
}

async fn run_turn(
    state: Arc<MockSessionState>,
    emitter: AgentEventEmitter,
    content: String,
    turn_id: String,
    cancel: CancelFlag,
) {
    emitter.emit(AgentEventType::TurnStarted, json!({"status": "running"}));

    let outcome = if content.contains("/tool") {
        run_tool_scenario(&state, &emitter, &turn_id, &cancel).await
    } else if content.contains("/fail") {
        run_fail_scenario(&emitter, &turn_id).await
    } else if content.contains("/slow") {
        run_slow_scenario(&state, &emitter, &turn_id, &cancel).await
    } else if content.contains("/never") {
        cancel.cancelled().await;
        TurnOutcome::Cancelled
    } else {
        run_echo_scenario(&state, &emitter, &turn_id, &cancel, &content).await
    };

    match outcome {
        TurnOutcome::Completed => {
            emitter.emit(
                AgentEventType::TurnCompleted,
                json!({"status": "completed"}),
            );
            emit_status(&emitter, SessionStatus::WaitingForInput);
        }
        TurnOutcome::Failed(error) => {
            emitter.emit(AgentEventType::TurnFailed, json!({"error": error.clone()}));
            emitter.emit(
                AgentEventType::SessionError,
                serde_json::to_value(SessionErrorPayload { error }).unwrap_or_default(),
            );
            emit_status(&emitter, SessionStatus::Failed);
        }
        TurnOutcome::Cancelled => {
            // The runtime (and `interrupt`) already emitted `turn.interrupted`
            // and the status transition; avoid a duplicate status event here.
        }
    }

    state.finish_turn(&turn_id);
    if emitter.current_turn_id().as_deref() == Some(turn_id.as_str()) {
        emitter.set_current_turn(None);
    }
}

enum TurnOutcome {
    Completed,
    Failed(ApiError),
    Cancelled,
}

async fn run_echo_scenario(
    state: &MockSessionState,
    emitter: &AgentEventEmitter,
    turn_id: &str,
    cancel: &CancelFlag,
    content: &str,
) -> TurnOutcome {
    let text = format!("Echo: {content}");
    let message = AgentMessage::new(
        emitter.session_id(),
        Some(turn_id.to_string()),
        MessageRole::Assistant,
        "",
    );
    let message_id = message.id.clone();
    emit_message_started(emitter, &message_id);
    let mut streamed = String::new();
    for chunk in text.split_inclusive(' ') {
        if cancel.is_cancelled() {
            complete_message(emitter, message, streamed);
            return TurnOutcome::Cancelled;
        }
        tokio::time::sleep(state.delay).await;
        streamed.push_str(chunk);
        emit_message_delta(emitter, &message_id, chunk);
    }
    if cancel.is_cancelled() {
        complete_message(emitter, message, streamed);
        return TurnOutcome::Cancelled;
    }
    complete_message(emitter, message, text);
    TurnOutcome::Completed
}

async fn run_slow_scenario(
    state: &MockSessionState,
    emitter: &AgentEventEmitter,
    turn_id: &str,
    cancel: &CancelFlag,
) -> TurnOutcome {
    let _ = state;
    let message = AgentMessage::new(
        emitter.session_id(),
        Some(turn_id.to_string()),
        MessageRole::Assistant,
        "",
    );
    let message_id = message.id.clone();
    emit_message_started(emitter, &message_id);
    let mut streamed = String::new();
    for index in 0..SLOW_WORDS {
        if cancel.is_cancelled() {
            complete_message(emitter, message, streamed);
            return TurnOutcome::Cancelled;
        }
        tokio::time::sleep(Duration::from_millis(SLOW_DELAY_MS)).await;
        let word = format!("word{} ", index + 1);
        streamed.push_str(&word);
        emit_message_delta(emitter, &message_id, &word);
    }
    if cancel.is_cancelled() {
        complete_message(emitter, message, streamed);
        return TurnOutcome::Cancelled;
    }
    complete_message(emitter, message, streamed);
    TurnOutcome::Completed
}

async fn run_fail_scenario(emitter: &AgentEventEmitter, turn_id: &str) -> TurnOutcome {
    let message = AgentMessage::new(
        emitter.session_id(),
        Some(turn_id.to_string()),
        MessageRole::Assistant,
        "Simulated failure",
    );
    let message_id = message.id.clone();
    emit_message_started(emitter, &message_id);
    emit_message_delta(emitter, &message_id, "Simulated failure");
    emitter.emit_with(
        AgentEventType::MessageCompleted,
        serde_json::to_value(MessageCompletedPayload {
            message: message.clone(),
        })
        .unwrap_or_default(),
        Some(turn_id.to_string()),
        Some(message_id),
        None,
    );
    TurnOutcome::Failed(ApiError::new(
        "mock_failure",
        "the mock provider was asked to fail",
    ))
}

async fn run_tool_scenario(
    state: &MockSessionState,
    emitter: &AgentEventEmitter,
    turn_id: &str,
    cancel: &CancelFlag,
) -> TurnOutcome {
    let mut message = AgentMessage::new(
        emitter.session_id(),
        Some(turn_id.to_string()),
        MessageRole::Assistant,
        "",
    );
    let message_id = message.id.clone();
    emit_message_started(emitter, &message_id);
    emit_message_delta(emitter, &message_id, "Running a tool...");

    let tool_id = new_id("tool");
    let input = json!({"command": "echo hello"});
    let tool = AgentToolCall {
        id: tool_id.clone(),
        session_id: emitter.session_id().to_string(),
        turn_id: Some(turn_id.to_string()),
        name: "Bash".to_string(),
        status: ToolCallStatus::Running,
        input: input.clone(),
        output: serde_json::Value::Null,
        error: None,
        started_at: now(),
        completed_at: None,
    };
    emitter.emit_with(
        AgentEventType::ToolStarted,
        serde_json::to_value(ToolEventPayload { tool: tool.clone() }).unwrap_or_default(),
        Some(turn_id.to_string()),
        Some(message_id.clone()),
        Some(tool_id.clone()),
    );

    let permission = AgentPermissionRequest {
        id: new_id("permission"),
        session_id: emitter.session_id().to_string(),
        turn_id: Some(turn_id.to_string()),
        tool_call_id: Some(tool_id.clone()),
        tool_name: "Bash".to_string(),
        description: "Allow Bash to run `echo hello`?".to_string(),
        input,
        status: PermissionStatus::Pending,
        created_at: now(),
        resolved_at: None,
        expires_at: None,
    };
    state
        .pending_permissions
        .lock()
        .insert(permission.id.clone());
    let receiver = emitter.request_permission(permission.clone());

    let decision = tokio::select! {
        _ = cancel.cancelled() => None,
        result = tokio::time::timeout(PERMISSION_TIMEOUT, receiver) => match result {
            Ok(Ok(decision)) => Some(decision),
            Ok(Err(_)) => None,
            Err(_) => {
                state.pending_permissions.lock().remove(&permission.id);
                state.emitter.resolve_permission_locally(
                    &permission.id,
                    PermissionStatus::TimedOut,
                );
                let mut failed_tool = tool.clone();
                failed_tool.status = ToolCallStatus::Failed;
                failed_tool.error = Some("permission request timed out".to_string());
                failed_tool.completed_at = Some(now());
                emitter.emit_with(
                    AgentEventType::ToolFailed,
                    serde_json::to_value(ToolStatusPayload {
                        tool_call_id: failed_tool.id.clone(),
                        status: ToolCallStatus::Failed,
                        error: failed_tool.error.clone(),
                    })
                    .unwrap_or_default(),
                    Some(turn_id.to_string()),
                    Some(message_id.clone()),
                    Some(failed_tool.id.clone()),
                );
                message.content = "Running a tool...\nPermission timed out.".to_string();
                complete_message(emitter, message, "Running a tool...\nPermission timed out.".to_string());
                return TurnOutcome::Failed(ApiError::new(
                    error_codes::TIMEOUT,
                    "mock permission request timed out",
                ));
            }
        }
    };
    let Some(decision) = decision else {
        state.pending_permissions.lock().remove(&permission.id);
        state
            .emitter
            .resolve_permission_locally(&permission.id, PermissionStatus::Cancelled);
        let partial = "Running a tool...".to_string();
        complete_message(emitter, message, partial);
        return TurnOutcome::Cancelled;
    };
    state.pending_permissions.lock().remove(&permission.id);

    if decision.approve {
        let mut completed_tool = tool;
        completed_tool.status = ToolCallStatus::Completed;
        completed_tool.output = json!({"stdout": "hello\n"});
        completed_tool.completed_at = Some(now());
        emitter.emit_with(
            AgentEventType::ToolCompleted,
            serde_json::to_value(ToolEventPayload {
                tool: completed_tool.clone(),
            })
            .unwrap_or_default(),
            Some(turn_id.to_string()),
            Some(message_id.clone()),
            Some(completed_tool.id.clone()),
        );
        let final_text = "Running a tool...\nTool finished.".to_string();
        message.content = final_text.clone();
        complete_message(emitter, message, final_text);
        TurnOutcome::Completed
    } else {
        let mut failed_tool = tool;
        failed_tool.status = ToolCallStatus::Failed;
        failed_tool.error = Some(
            decision
                .message
                .unwrap_or_else(|| "rejected by user".to_string()),
        );
        failed_tool.completed_at = Some(now());
        emitter.emit_with(
            AgentEventType::ToolFailed,
            serde_json::to_value(ToolStatusPayload {
                tool_call_id: failed_tool.id.clone(),
                status: ToolCallStatus::Failed,
                error: failed_tool.error.clone(),
            })
            .unwrap_or_default(),
            Some(turn_id.to_string()),
            Some(message_id.clone()),
            Some(failed_tool.id.clone()),
        );
        let final_text = "Running a tool...\nTool rejected.".to_string();
        message.content = final_text.clone();
        complete_message(emitter, message, final_text);
        TurnOutcome::Completed
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

fn emit_message_started(emitter: &AgentEventEmitter, message_id: &str) {
    emitter.emit_with(
        AgentEventType::MessageStarted,
        serde_json::to_value(MessageStartedPayload {
            message_id: message_id.to_string(),
            role: MessageRole::Assistant,
        })
        .unwrap_or_default(),
        None,
        Some(message_id.to_string()),
        None,
    );
}

fn emit_message_delta(emitter: &AgentEventEmitter, message_id: &str, text: &str) {
    emitter.emit_with(
        AgentEventType::MessageDelta,
        serde_json::to_value(MessageDeltaPayload {
            message_id: message_id.to_string(),
            text: text.to_string(),
            index: None,
        })
        .unwrap_or_default(),
        None,
        Some(message_id.to_string()),
        None,
    );
}

fn complete_message(emitter: &AgentEventEmitter, mut message: AgentMessage, content: String) {
    message.content = content;
    let message_id = message.id.clone();
    emitter.emit_with(
        AgentEventType::MessageCompleted,
        serde_json::to_value(MessageCompletedPayload {
            message: message.clone(),
        })
        .unwrap_or_default(),
        message.turn_id.clone(),
        Some(message_id),
        None,
    );
}

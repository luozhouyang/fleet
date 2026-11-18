//! OpenCode provider adapter: HTTP agent admin + SSE event stream.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use fleet_core_common::{now, Timestamp};
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEventType, AgentMessage, AgentSession, AgentStartConfig, AgentToolCall,
    MessageCompletedPayload, MessageDeltaPayload, MessageRole, MessageStartedPayload,
    PermissionDecision, PermissionStatus, ProviderInfo, ProviderKind, QuestionStatus,
    SessionErrorPayload, SessionStatus, StatusChangedPayload, ToolCallStatus, ToolEventPayload,
};
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

use crate::error::{Result, RuntimeError};
use crate::event::AgentEventEmitter;
use crate::provider::{AgentProvider, ProviderContext};

use super::client::{NativeSessionInfo, OpenCodeClient};
use super::mapper::{self, MappedEvent, ToolPart};
use super::server::OpenCodeServer;

const PERMISSION_TIMEOUT: Duration = Duration::from_secs(600);
const QUESTION_TIMEOUT: Duration = Duration::from_secs(600);
const SSE_RECONNECT_DELAY: Duration = Duration::from_secs(1);

struct OpenMessage {
    id: String,
    text: String,
}

struct ToolRecord {
    name: String,
    input: Value,
    started_at: Timestamp,
    status: ToolCallStatus,
}

#[derive(Default)]
struct StreamState {
    current_message: Option<OpenMessage>,
    started_messages: HashSet<String>,
    tools: HashMap<String, ToolRecord>,
    status: Option<SessionStatus>,
}

struct NativeSession {
    unified_id: String,
    native_id: String,
    emitter: AgentEventEmitter,
    state: Mutex<StreamState>,
    permission_tasks: Mutex<Vec<JoinHandle<()>>>,
    question_tasks: Mutex<Vec<JoinHandle<()>>>,
}

struct Inner {
    bin: String,
    configured: Option<(String, Option<String>)>,
    server: AsyncMutex<Option<Arc<OpenCodeServer>>>,
    sessions: DashMap<String, Arc<NativeSession>>,
    native_index: DashMap<String, String>,
    sse_started: AtomicBool,
    sse_status: watch::Sender<bool>,
}

impl Inner {
    async fn endpoint(&self) -> Result<Arc<OpenCodeServer>> {
        let mut guard = self.server.lock().await;
        if let Some(server) = guard.as_ref() {
            if server.is_running() {
                return Ok(Arc::clone(server));
            }
            tracing::warn!("opencode server exited; starting a new one");
            *guard = None;
        }
        let server = match &self.configured {
            Some((url, password)) => {
                Arc::new(OpenCodeServer::connect(url.clone(), password.clone()))
            }
            None => Arc::new(OpenCodeServer::spawn(&self.bin).await?),
        };
        *guard = Some(Arc::clone(&server));
        Ok(server)
    }

    async fn client(&self) -> Result<OpenCodeClient> {
        let server = self.endpoint().await?;
        Ok(OpenCodeClient::new(
            server.base_url().to_string(),
            server.password().to_string(),
        ))
    }

    fn session(&self, unified_id: &str) -> Result<Arc<NativeSession>> {
        self.sessions
            .get(unified_id)
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| RuntimeError::SessionNotFound(unified_id.to_string()))
    }
}

/// OpenCode provider.
pub struct OpenCodeProvider {
    inner: Arc<Inner>,
}

impl OpenCodeProvider {
    /// Launch a private `opencode serve` for this daemon on first use.
    pub fn new(bin: impl Into<String>) -> Self {
        let bin = bin.into();
        Self {
            inner: Arc::new(Inner {
                configured: None,
                bin,
                server: AsyncMutex::new(None),
                sessions: DashMap::new(),
                native_index: DashMap::new(),
                sse_started: AtomicBool::new(false),
                sse_status: watch::channel(false).0,
            }),
        }
    }

    /// Connect to an already running OpenCode server.
    pub fn with_server(url: impl Into<String>, password: Option<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                bin: String::new(),
                configured: Some((url.into(), password)),
                server: AsyncMutex::new(None),
                sessions: DashMap::new(),
                native_index: DashMap::new(),
                sse_started: AtomicBool::new(false),
                sse_status: watch::channel(false).0,
            }),
        }
    }

    /// Read `OPENCODE_SERVER_URL` / `OPENCODE_SERVER_PASSWORD`.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("OPENCODE_SERVER_URL").ok()?;
        if url.trim().is_empty() {
            return None;
        }
        Some(Self::with_server(
            url,
            std::env::var("OPENCODE_SERVER_PASSWORD").ok(),
        ))
    }

    async fn ensure_sse_ready(&self) -> Result<()> {
        let mut status = self.inner.sse_status.subscribe();
        if !self.inner.sse_started.swap(true, Ordering::SeqCst) {
            let weak = Arc::downgrade(&self.inner);
            tokio::spawn(async move { sse_loop(weak).await });
        }
        if *status.borrow() {
            return Ok(());
        }
        let wait = async {
            while !*status.borrow_and_update() {
                if status.changed().await.is_err() {
                    break;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .map_err(|_| RuntimeError::Timeout("opencode event stream did not connect".into()))?;
        if *status.borrow() {
            Ok(())
        } else {
            Err(RuntimeError::ProviderUnavailable(
                "opencode event stream is unavailable".into(),
            ))
        }
    }
}

#[async_trait]
impl AgentProvider for OpenCodeProvider {
    fn provider_id(&self) -> &'static str {
        "opencode"
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.provider_id().to_string(),
            kind: ProviderKind::OpenCode,
            display_name: "OpenCode".to_string(),
            version: None,
            available: self.inner.configured.is_some()
                || crate::provider::binary_exists(&self.inner.bin),
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
            structured_diff: false,
            session_persistence: true,
            tool_calls: true,
            images: false,
        }
    }

    fn agent_type(&self) -> &'static str {
        "opencode"
    }

    fn executable(&self) -> Option<&str> {
        if self.inner.configured.is_some() {
            None
        } else {
            Some(&self.inner.bin)
        }
    }

    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession> {
        let cwd = resolve_cwd(&config.cwd).await?;
        let client = self.inner.client().await?;

        let (agent, model) = opencode_options(&config);
        let info = client
            .create_session(
                &cwd.to_string_lossy(),
                config.title.as_deref(),
                agent.as_deref(),
                model.as_ref(),
            )
            .await?;
        let NativeSessionInfo { id: native_id, .. } = info;

        let session = Arc::new(NativeSession {
            unified_id: ctx.session.id.clone(),
            native_id: native_id.clone(),
            emitter: ctx.emitter.clone(),
            state: Mutex::new(StreamState::default()),
            permission_tasks: Mutex::new(Vec::new()),
            question_tasks: Mutex::new(Vec::new()),
        });
        self.inner
            .sessions
            .insert(ctx.session.id.clone(), Arc::clone(&session));
        self.inner
            .native_index
            .insert(native_id.clone(), ctx.session.id.clone());

        ctx.emitter.emit(
            AgentEventType::AgentRawEvent,
            json!({ "provider_session_id": native_id }),
        );
        ctx.emitter.emit(AgentEventType::SessionStarted, json!({}));
        set_status(&session, SessionStatus::Running);

        if let Err(error) = self.ensure_sse_ready().await {
            self.inner.sessions.remove(&ctx.session.id);
            self.inner.native_index.remove(&native_id);
            if let Ok(client) = self.inner.client().await {
                if let Err(cleanup) = client.delete_session(&native_id).await {
                    tracing::warn!(error = %cleanup, "failed to delete opencode session after start failure");
                }
            }
            return Err(error);
        }

        let mut session_record = ctx.session;
        session_record.status = SessionStatus::Running;
        session_record.updated_at = now();
        if !session_record.metadata.is_object() {
            session_record.metadata = json!({});
        }
        session_record.metadata["provider_session_id"] = json!(native_id);
        Ok(session_record)
    }

    async fn send_message(&self, session_id: &str, content: String) -> Result<()> {
        if content.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "message content must not be empty".into(),
            ));
        }
        let session = self.inner.session(session_id)?;
        let client = self.inner.client().await?;
        client.prompt(&session.native_id, &content).await
    }

    async fn interrupt(&self, session_id: &str) -> Result<()> {
        let session = self.inner.session(session_id)?;
        let client = self.inner.client().await?;
        // The runtime emits the unified turn/status transition; the SSE
        // `session.execution.interrupted` event only updates local state.
        let _ = client.interrupt(&session.native_id).await?;
        Ok(())
    }

    async fn resume(&self, session_id: &str) -> Result<()> {
        let session = self.inner.session(session_id)?;
        session
            .emitter
            .emit(AgentEventType::SessionResumed, json!({}));
        set_status(&session, SessionStatus::Running);
        Ok(())
    }

    async fn shutdown(&self, session_id: &str) -> Result<()> {
        let Some((_, session)) = self.inner.sessions.remove(session_id) else {
            return Err(RuntimeError::SessionNotFound(session_id.to_string()));
        };
        self.inner.native_index.remove(&session.native_id);

        {
            let mut tasks = session.permission_tasks.lock();
            for task in tasks.drain(..) {
                task.abort();
            }
        }
        {
            let mut tasks = session.question_tasks.lock();
            for task in tasks.drain(..) {
                task.abort();
            }
        }

        if let Ok(client) = self.inner.client().await {
            if let Err(err) = client.delete_session(&session.native_id).await {
                tracing::warn!(session = %session_id, error = %err, "failed to delete opencode session");
            }
        }

        complete_current_message(&session);
        session
            .emitter
            .emit(AgentEventType::SessionTerminated, json!({}));
        set_status(&session, SessionStatus::Terminated);
        Ok(())
    }

    async fn respond_permission(
        &self,
        _session_id: &str,
        decision: PermissionDecision,
    ) -> Result<()> {
        // Permissions emitted through `AgentEventEmitter::request_permission`
        // are answered by the task spawned in `dispatch_event`. This fallback is
        // only reached if the runtime has no sender for the request.
        tracing::debug!(
            permission = %decision.permission_id,
            "opencode permission has no native fallback channel"
        );
        Ok(())
    }
    async fn respond_question(
        &self,
        _session_id: &str,
        _decision: fleet_core_protocol::agent::QuestionDecision,
    ) -> Result<()> {
        // The task spawned in `dispatch_event` owns the receiver and posts the
        // answer to OpenCode; there is no second channel.
        Ok(())
    }
}

async fn sse_loop(weak: Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else {
            break;
        };
        let client = match inner.client().await {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(error = %err, "opencode event stream unavailable");
                drop(inner);
                tokio::time::sleep(SSE_RECONNECT_DELAY).await;
                continue;
            }
        };
        match client.event_stream().await {
            Ok(response) => {
                let _ = inner.sse_status.send(true);
                if let Err(err) = consume_sse(&inner, response).await {
                    tracing::warn!(error = %err, "opencode event stream ended");
                }
                let _ = inner.sse_status.send(false);
            }
            Err(err) => {
                let _ = inner.sse_status.send(false);
                tracing::warn!(error = %err, "opencode event stream failed");
            }
        }
        drop(inner);
        tokio::time::sleep(SSE_RECONNECT_DELAY).await;
    }
}

async fn consume_sse(inner: &Arc<Inner>, response: reqwest::Response) -> Result<()> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut data_lines: Vec<String> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| {
            RuntimeError::ProviderUnavailable(format!("opencode SSE chunk failed: {err}"))
        })?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(position) = buffer.find('\n') {
            let line: String = buffer.drain(..=position).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !data_lines.is_empty() {
                    let payload = data_lines.join("\n");
                    data_lines.clear();
                    dispatch_payload(inner, &payload).await;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start().to_string());
            }
            // Comments (`: heartbeat`) and other SSE fields are ignored.
        }
    }

    if !data_lines.is_empty() {
        dispatch_payload(inner, &data_lines.join("\n")).await;
    }
    Ok(())
}

async fn dispatch_payload(inner: &Arc<Inner>, payload: &str) {
    let value: Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(err) => {
            tracing::debug!(error = %err, "ignoring malformed opencode event");
            return;
        }
    };
    let Some(parsed) = mapper::map_event(&value) else {
        return;
    };
    let Some(native_id) = parsed.session_id.as_deref() else {
        return;
    };
    let Some(unified_id) = inner
        .native_index
        .get(native_id)
        .map(|entry| entry.value().clone())
    else {
        return;
    };
    let Some(session) = inner
        .sessions
        .get(&unified_id)
        .map(|entry| Arc::clone(entry.value()))
    else {
        return;
    };
    dispatch_event(inner, &session, parsed.mapped).await;
}

async fn dispatch_event(inner: &Arc<Inner>, session: &Arc<NativeSession>, mapped: MappedEvent) {
    match mapped {
        MappedEvent::TextStarted { message_id, .. } => {
            start_message(session, &message_id);
        }
        MappedEvent::TextDelta {
            message_id,
            delta,
            ordinal,
        } => {
            start_message(session, &message_id);
            {
                let mut state = session.state.lock();
                if let Some(message) = state.current_message.as_mut() {
                    if message.id == message_id {
                        message.text.push_str(&delta);
                    }
                }
            }
            session.emitter.emit_with(
                AgentEventType::MessageDelta,
                serde_json::to_value(MessageDeltaPayload {
                    message_id: message_id.clone(),
                    text: delta,
                    index: Some(ordinal as u32),
                })
                .unwrap_or_default(),
                None,
                Some(message_id),
                None,
            );
        }
        MappedEvent::TextEnded {
            message_id, text, ..
        } => {
            start_message(session, &message_id);
            let mut state = session.state.lock();
            if let Some(message) = state.current_message.as_mut() {
                if message.id == message_id && message.text.is_empty() && !text.is_empty() {
                    message.text = text;
                }
            }
        }
        MappedEvent::ToolCalled { id, name, input } => {
            start_tool(session, &id, &name, input);
        }
        MappedEvent::ToolSucceeded { id, content } => {
            finish_tool(session, &id, ToolCallStatus::Completed, content, None);
        }
        MappedEvent::ToolFailed { id, error } => {
            finish_tool(
                session,
                &id,
                ToolCallStatus::Failed,
                Value::Null,
                Some(error),
            );
        }
        MappedEvent::ContentUpdated { content } => {
            for part in mapper::tool_parts(&content) {
                match part {
                    ToolPart::Running { id, name, input } => {
                        start_tool(session, &id, &name, input);
                    }
                    ToolPart::Completed {
                        id,
                        name,
                        input,
                        content,
                    } => {
                        start_tool(session, &id, &name, input);
                        finish_tool(session, &id, ToolCallStatus::Completed, content, None);
                    }
                    ToolPart::Failed {
                        id,
                        name,
                        input,
                        error,
                    } => {
                        start_tool(session, &id, &name, input);
                        finish_tool(
                            session,
                            &id,
                            ToolCallStatus::Failed,
                            Value::Null,
                            Some(error),
                        );
                    }
                }
            }
        }
        MappedEvent::ExecutionStarted => {
            let mut state = session.state.lock();
            state.status = Some(SessionStatus::Running);
        }
        MappedEvent::ExecutionSucceeded => {
            complete_current_message(session);
            session.emitter.emit(
                AgentEventType::TurnCompleted,
                json!({"status": "completed"}),
            );
            set_status(session, SessionStatus::WaitingForInput);
        }
        MappedEvent::ExecutionInterrupted { .. } => {
            complete_current_message(session);
            session
                .emitter
                .emit(AgentEventType::TurnInterrupted, json!({"reason": "user"}));
            // The runtime already emitted the status for user-initiated
            // interrupts; keep the local view in sync without a second event.
            session.state.lock().status = Some(SessionStatus::Interrupted);
        }
        MappedEvent::ExecutionFailed => {
            complete_current_message(session);
            let error = fleet_core_common::ApiError::new(
                "opencode_execution_failed",
                "OpenCode reported a failed execution",
            );
            session
                .emitter
                .emit(AgentEventType::TurnFailed, json!({"error": error.message}));
            session.emitter.emit(
                AgentEventType::SessionError,
                serde_json::to_value(SessionErrorPayload { error }).unwrap_or_default(),
            );
            set_status(session, SessionStatus::Failed);
        }
        MappedEvent::PermissionAsked { request } => {
            let mut request = request;
            request.session_id = session.unified_id.clone();
            let receiver = session.emitter.request_permission(request.clone());
            let native_id = session.native_id.clone();
            let request_id = request.id.clone();
            let client = match inner.client().await {
                Ok(client) => client,
                Err(err) => {
                    tracing::warn!(error = %err, "cannot reply to opencode permission");
                    return;
                }
            };
            let emitter = session.emitter.clone();
            let unified_id = request.id.clone();
            let task = tokio::spawn(async move {
                match tokio::time::timeout(PERMISSION_TIMEOUT, receiver).await {
                    Ok(Ok(decision)) => {
                        if let Err(err) = client
                            .reply_permission(
                                &native_id,
                                &request_id,
                                decision.approve,
                                decision.message.as_deref(),
                            )
                            .await
                        {
                            tracing::warn!(error = %err, "opencode permission reply failed");
                        }
                    }
                    Ok(Err(_)) => {}
                    Err(_) => {
                        emitter.resolve_permission_locally(&unified_id, PermissionStatus::TimedOut);
                        let _ = client
                            .reply_permission(
                                &native_id,
                                &request_id,
                                false,
                                Some("permission timed out in agent-fleet"),
                            )
                            .await;
                    }
                }
            });
            session.permission_tasks.lock().push(task);
        }
        MappedEvent::PermissionReplied { .. } => {
            // The runtime owns the unified approved/rejected event.
        }
        MappedEvent::QuestionAsked {
            request,
            native_request_id,
        } => {
            let mut request = request;
            request.session_id = session.unified_id.clone();
            let receiver = session.emitter.request_question(request.clone());
            let client = match inner.client().await {
                Ok(client) => client,
                Err(err) => {
                    tracing::warn!(error = %err, "cannot reply to opencode question");
                    session
                        .emitter
                        .resolve_question_locally(&request.id, QuestionStatus::DeliveryFailed);
                    return;
                }
            };
            let native_id = session.native_id.clone();
            let request_id = native_request_id;
            let question_id = request.id.clone();
            let question_ids: Vec<String> = request
                .questions
                .iter()
                .map(|question| question.id.clone())
                .collect();
            let emitter = session.emitter.clone();
            let task = tokio::spawn(async move {
                match tokio::time::timeout(QUESTION_TIMEOUT, receiver).await {
                    Ok(Ok(decision)) => {
                        if decision.reject {
                            if let Err(err) = client.reject_question(&native_id, &request_id).await
                            {
                                tracing::warn!(error = %err, "opencode question reject failed");
                                emitter.fail_question_delivery(&question_id);
                            }
                            return;
                        }
                        let answers = question_answers(&question_ids, &decision);
                        if let Err(err) = client
                            .reply_question(&native_id, &request_id, answers)
                            .await
                        {
                            tracing::warn!(error = %err, "opencode question reply failed");
                            emitter.fail_question_delivery(&question_id);
                        }
                    }
                    Ok(Err(_)) => {
                        emitter.resolve_question_locally(&question_id, QuestionStatus::Cancelled);
                        let _ = client.reject_question(&native_id, &request_id).await;
                    }
                    Err(_) => {
                        emitter.resolve_question_locally(&question_id, QuestionStatus::TimedOut);
                        let _ = client.reject_question(&native_id, &request_id).await;
                    }
                }
            });
            session.question_tasks.lock().push(task);
        }
        MappedEvent::QuestionReplied { .. } | MappedEvent::QuestionRejected { .. } => {
            // The runtime owns the unified answered/rejected event.
        }
        MappedEvent::Idle => {
            let should_emit = {
                let state = session.state.lock();
                !matches!(
                    state.status,
                    Some(
                        SessionStatus::Terminated
                            | SessionStatus::Failed
                            | SessionStatus::Interrupted
                            | SessionStatus::Completed
                    )
                )
            };
            if should_emit {
                set_status(session, SessionStatus::WaitingForInput);
            }
        }
        MappedEvent::SessionDeleted => {
            complete_current_message(session);
            session
                .emitter
                .emit(AgentEventType::SessionTerminated, json!({}));
            set_status(session, SessionStatus::Terminated);
            inner.sessions.remove(&session.unified_id);
            inner.native_index.remove(&session.native_id);
        }
        MappedEvent::Other { .. } => {}
    }
}

/// Collapse a unified decision into one answer list per question, in order.
/// Free-text custom answers are appended to the selected labels.
fn question_answers(
    question_ids: &[String],
    decision: &fleet_core_protocol::agent::QuestionDecision,
) -> Vec<Vec<String>> {
    question_ids
        .iter()
        .map(|question_id| {
            decision
                .answers
                .iter()
                .find(|answer| &answer.id == question_id)
                .map(|answer| {
                    let mut selected = answer.selected.clone();
                    if let Some(custom) = answer.custom.as_deref().map(str::trim) {
                        if !custom.is_empty() && !selected.iter().any(|item| item == custom) {
                            selected.push(custom.to_string());
                        }
                    }
                    selected
                })
                .unwrap_or_default()
        })
        .collect()
}

fn start_message(session: &NativeSession, message_id: &str) {
    // A new assistant message id means the previous one is complete; emit its
    // terminal event before replacing it.
    let previous_incomplete = {
        let state = session.state.lock();
        state
            .current_message
            .as_ref()
            .is_some_and(|message| message.id != message_id)
    };
    if previous_incomplete {
        complete_current_message(session);
    }
    let newly_started = {
        let mut state = session.state.lock();
        let already_started = !state.started_messages.insert(message_id.to_string());
        let reuse = state
            .current_message
            .as_ref()
            .map(|message| message.id == message_id)
            .unwrap_or(false);
        if !reuse {
            state.current_message = Some(OpenMessage {
                id: message_id.to_string(),
                text: String::new(),
            });
        }
        !already_started
    };
    if newly_started {
        session.emitter.emit_with(
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
}

fn complete_current_message(session: &NativeSession) {
    let message = {
        let mut state = session.state.lock();
        let current = state.current_message.take();
        if let Some(current) = &current {
            state.started_messages.remove(&current.id);
        }
        current
    };
    let Some(OpenMessage { id, text }) = message else {
        return;
    };
    let message = AgentMessage {
        id: id.clone(),
        session_id: session.unified_id.clone(),
        turn_id: session.emitter.current_turn_id(),
        role: MessageRole::Assistant,
        content: text,
        created_at: now(),
    };
    session.emitter.emit_with(
        AgentEventType::MessageCompleted,
        serde_json::to_value(MessageCompletedPayload { message }).unwrap_or_default(),
        None,
        Some(id),
        None,
    );
}

fn start_tool(session: &NativeSession, id: &str, name: &str, input: Value) {
    let emit = {
        let mut state = session.state.lock();
        match state.tools.get_mut(id) {
            Some(record) => {
                if record.input.is_null() && !input.is_null() {
                    record.input = input;
                }
                if !record.name.is_empty() && record.name == "tool" && name != "tool" {
                    record.name = name.to_string();
                }
                false
            }
            None => {
                state.tools.insert(
                    id.to_string(),
                    ToolRecord {
                        name: name.to_string(),
                        input,
                        started_at: now(),
                        status: ToolCallStatus::Running,
                    },
                );
                true
            }
        }
    };
    if emit {
        if let Some(tool) = tool_snapshot(session, id, ToolCallStatus::Running, Value::Null, None) {
            session.emitter.emit_with(
                AgentEventType::ToolStarted,
                serde_json::to_value(ToolEventPayload { tool }).unwrap_or_default(),
                None,
                None,
                Some(id.to_string()),
            );
        }
    }
}

fn finish_tool(
    session: &NativeSession,
    id: &str,
    status: ToolCallStatus,
    output: Value,
    error: Option<String>,
) {
    let newly_started = {
        let mut state = session.state.lock();
        if !state.tools.contains_key(id) {
            if let Some(message) = state.current_message.as_ref() {
                let _ = message;
            }
            state.tools.insert(
                id.to_string(),
                ToolRecord {
                    name: "tool".to_string(),
                    input: Value::Null,
                    started_at: now(),
                    status: ToolCallStatus::Running,
                },
            );
            true
        } else {
            false
        }
    };
    if newly_started {
        if let Some(tool) = tool_snapshot(session, id, ToolCallStatus::Running, Value::Null, None) {
            session.emitter.emit_with(
                AgentEventType::ToolStarted,
                serde_json::to_value(ToolEventPayload { tool }).unwrap_or_default(),
                None,
                None,
                Some(id.to_string()),
            );
        }
    }

    let emit = {
        let mut state = session.state.lock();
        match state.tools.get_mut(id) {
            Some(record) => {
                if matches!(
                    record.status,
                    ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Cancelled
                ) {
                    false
                } else {
                    record.status = status;
                    true
                }
            }
            None => false,
        }
    };
    if emit {
        if let Some(tool) = tool_snapshot(session, id, status, output, error) {
            let event = if status == ToolCallStatus::Completed {
                AgentEventType::ToolCompleted
            } else {
                AgentEventType::ToolFailed
            };
            session.emitter.emit_with(
                event,
                serde_json::to_value(ToolEventPayload { tool }).unwrap_or_default(),
                None,
                None,
                Some(id.to_string()),
            );
        }
    }
}

fn tool_snapshot(
    session: &NativeSession,
    id: &str,
    status: ToolCallStatus,
    output: Value,
    error: Option<String>,
) -> Option<AgentToolCall> {
    let state = session.state.lock();
    let record = state.tools.get(id)?;
    Some(AgentToolCall {
        id: id.to_string(),
        session_id: session.unified_id.clone(),
        turn_id: session.emitter.current_turn_id(),
        name: record.name.clone(),
        status,
        input: record.input.clone(),
        output,
        error,
        started_at: record.started_at,
        completed_at: if matches!(
            status,
            ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Cancelled
        ) {
            Some(now())
        } else {
            None
        },
    })
}

fn set_status(session: &NativeSession, status: SessionStatus) {
    let changed = {
        let mut state = session.state.lock();
        if state.status == Some(status) {
            false
        } else {
            state.status = Some(status);
            true
        }
    };
    if changed {
        session.emitter.emit(
            AgentEventType::AgentStatusChanged,
            serde_json::to_value(StatusChangedPayload {
                status,
                reason: None,
            })
            .unwrap_or_default(),
        );
    }
}

async fn resolve_cwd(raw: &str) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "opencode sessions require a working directory".into(),
        ));
    }
    let path = tokio::fs::canonicalize(raw)
        .await
        .map_err(|err| RuntimeError::InvalidRequest(format!("invalid cwd `{raw}`: {err}")))?;
    if !path.is_dir() {
        return Err(RuntimeError::InvalidRequest(format!(
            "cwd `{raw}` is not a directory"
        )));
    }
    Ok(path)
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_protocol::agent::{AgentQuestionAnswer, QuestionDecision};

    #[test]
    fn question_answers_follow_question_order_and_include_custom() {
        let question_ids = vec!["que_1_q0".to_string(), "que_1_q1".to_string()];
        let decision = QuestionDecision {
            question_id: "que_1".to_string(),
            session_id: Some("session_1".to_string()),
            answers: vec![
                AgentQuestionAnswer {
                    id: "que_1_q1".to_string(),
                    selected: vec!["production".to_string()],
                    custom: Some("  eu-west-1  ".to_string()),
                },
                AgentQuestionAnswer {
                    id: "que_1_q0".to_string(),
                    selected: vec!["staging".to_string()],
                    custom: None,
                },
            ],
            reject: false,
            message: None,
        };

        assert_eq!(
            question_answers(&question_ids, &decision),
            vec![
                vec!["staging".to_string()],
                vec!["production".to_string(), "eu-west-1".to_string()],
            ]
        );
    }

    #[test]
    fn unanswered_question_yields_empty_answer_list() {
        let question_ids = vec!["que_1_q0".to_string(), "que_1_q1".to_string()];
        let decision = QuestionDecision {
            question_id: "que_1".to_string(),
            session_id: None,
            answers: vec![AgentQuestionAnswer {
                id: "que_1_q0".to_string(),
                selected: vec!["staging".to_string()],
                custom: None,
            }],
            reject: false,
            message: None,
        };

        let answers = question_answers(&question_ids, &decision);
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0], vec!["staging".to_string()]);
        assert!(answers[1].is_empty());
    }
}

fn opencode_options(config: &AgentStartConfig) -> (Option<String>, Option<Value>) {
    let extra = config.extra.get("opencode");
    let agent = extra
        .and_then(|value| value.get("agent"))
        .and_then(Value::as_str)
        .or_else(|| config.extra.get("agent").and_then(Value::as_str))
        .map(str::to_string);
    let model = extra
        .and_then(|value| value.get("model"))
        .cloned()
        .or_else(|| config.extra.get("model").cloned())
        .or_else(|| {
            config.model.as_deref().and_then(|raw| {
                raw.split_once('/')
                    .map(|(provider, id)| json!({ "providerID": provider, "id": id }))
            })
        });
    (agent, model)
}

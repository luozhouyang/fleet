//! Generic PTY adapter for terminal-compatible CLIs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use fleet_core_common::now;
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEventType, AgentMessage, AgentSession, AgentStartConfig,
    MessageCompletedPayload, MessageDeltaPayload, MessageRole, MessageStartedPayload,
    PermissionDecision, ProviderInfo, ProviderKind, SessionErrorPayload, SessionStatus,
    StatusChangedPayload,
};
use fleet_daemon_process::{CommandSpec, PtyProcess};
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::error::{Result, RuntimeError};
use crate::event::AgentEventEmitter;
use crate::provider::{AgentProvider, ProviderContext};

struct GenericSession {
    process: AsyncMutex<PtyProcess>,
    emitter: AgentEventEmitter,
    message_id: Mutex<Option<String>>,
    content: Mutex<String>,
    shutting_down: AtomicBool,
    reader: Mutex<Option<JoinHandle<()>>>,
}

/// PTY-backed provider for agents that only expose a terminal interface.
pub struct GenericProvider {
    bin: String,
    args: Vec<String>,
    sessions: Mutex<HashMap<String, Arc<GenericSession>>>,
}

impl GenericProvider {
    /// Create a provider that runs `bin` with `args` inside a pseudo-terminal.
    pub fn new(bin: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            bin: bin.into(),
            args: args.into_iter().map(Into::into).collect(),
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl AgentProvider for GenericProvider {
    fn provider_id(&self) -> &'static str {
        "generic"
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.provider_id().to_string(),
            kind: ProviderKind::Generic,
            display_name: "Generic CLI (PTY)".to_string(),
            version: None,
            available: crate::provider::binary_exists(&self.bin),
            capabilities: self.capabilities(),
            models: Vec::new(),
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            interrupt: true,
            resume: false,
            permission_approval: false,
            user_questions: false,
            structured_events: false,
            structured_diff: false,
            session_persistence: false,
            tool_calls: false,
            images: false,
        }
    }

    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession> {
        let mut spec = CommandSpec::new(&self.bin).args(self.args.clone());
        if !config.cwd.is_empty() {
            spec = spec.cwd(config.cwd.clone());
        }
        for (key, value) in &config.env {
            spec = spec.env(key.clone(), value.clone());
        }

        let process = PtyProcess::spawn(spec, 120, 40)?;
        let mut reader = process.reader();

        let session = Arc::new(GenericSession {
            process: AsyncMutex::new(process),
            emitter: ctx.emitter.clone(),
            message_id: Mutex::new(None),
            content: Mutex::new(String::new()),
            shutting_down: AtomicBool::new(false),
            reader: Mutex::new(None),
        });

        let reader_session = Arc::clone(&session);
        let task = tokio::spawn(async move {
            pump_output(reader_session, &mut reader).await;
        });
        *session.reader.lock() = Some(task);

        self.sessions
            .lock()
            .insert(ctx.session.id.clone(), Arc::clone(&session));

        ctx.emitter.emit(AgentEventType::SessionStarted, json!({}));
        emit_status(&ctx.emitter, SessionStatus::Running);

        let mut started = ctx.session;
        started.status = SessionStatus::Running;
        started.updated_at = now();
        Ok(started)
    }

    async fn send_message(&self, session_id: &str, content: String) -> Result<()> {
        let session = self.session(session_id)?;
        let mut process = session.process.lock().await;
        let mut input = content.into_bytes();
        input.push(b'\r');
        process.write(&input)?;
        Ok(())
    }

    async fn interrupt(&self, session_id: &str) -> Result<()> {
        let session = self.session(session_id)?;
        let mut process = session.process.lock().await;
        process.write(&[0x03])?;
        session
            .emitter
            .emit(AgentEventType::TurnInterrupted, json!({"reason": "ctrl-c"}));
        emit_status(&session.emitter, SessionStatus::Interrupted);
        Ok(())
    }

    async fn resume(&self, _session_id: &str) -> Result<()> {
        Err(RuntimeError::Unsupported(
            "a PTY session cannot be resumed".to_string(),
        ))
    }

    async fn shutdown(&self, session_id: &str) -> Result<()> {
        let session = self
            .sessions
            .lock()
            .remove(session_id)
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))?;
        session.shutting_down.store(true, Ordering::SeqCst);
        if let Some(task) = session.reader.lock().take() {
            task.abort();
        }
        {
            let mut process = session.process.lock().await;
            let _ = process.kill();
            let _ = process.wait();
        }
        session
            .emitter
            .emit(AgentEventType::SessionTerminated, json!({}));
        emit_status(&session.emitter, SessionStatus::Terminated);
        Ok(())
    }

    async fn respond_permission(
        &self,
        _session_id: &str,
        _decision: PermissionDecision,
    ) -> Result<()> {
        Err(RuntimeError::Unsupported(
            "PTY sessions do not expose structured permission requests".to_string(),
        ))
    }
    async fn respond_question(
        &self,
        _session_id: &str,
        _decision: fleet_core_protocol::agent::QuestionDecision,
    ) -> Result<()> {
        Err(RuntimeError::Unsupported(
            "PTY sessions do not expose structured user questions".to_string(),
        ))
    }

}

impl GenericProvider {
    fn session(&self, session_id: &str) -> Result<Arc<GenericSession>> {
        self.sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.to_string()))
    }
}

async fn pump_output(
    session: Arc<GenericSession>,
    reader: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    let mut started = false;
    while let Some(chunk) = reader.recv().await {
        let text = String::from_utf8_lossy(&chunk).to_string();
        let message_id = {
            let mut guard = session.message_id.lock();
            if guard.is_none() {
                let message = AgentMessage::new(
                    session.emitter.session_id(),
                    session.emitter.current_turn_id(),
                    MessageRole::Assistant,
                    "",
                );
                let id = message.id.clone();
                *guard = Some(id.clone());
                session.emitter.emit_with(
                    AgentEventType::MessageStarted,
                    serde_json::to_value(MessageStartedPayload {
                        message_id: id.clone(),
                        role: MessageRole::Assistant,
                    })
                    .unwrap_or_default(),
                    None,
                    Some(id.clone()),
                    None,
                );
                started = true;
                id
            } else {
                guard.clone().unwrap_or_default()
            }
        };

        session.content.lock().push_str(&text);
        session.emitter.emit_with(
            AgentEventType::MessageDelta,
            serde_json::to_value(MessageDeltaPayload {
                message_id: message_id.clone(),
                text: text.clone(),
                index: None,
            })
            .unwrap_or_default(),
            None,
            Some(message_id.clone()),
            None,
        );
        session.emitter.emit(
            AgentEventType::AgentRawEvent,
            json!({"chunk": text.clone()}),
        );
    }

    if started {
        let content = session.content.lock().clone();
        let message = AgentMessage::new(
            session.emitter.session_id(),
            session.emitter.current_turn_id(),
            MessageRole::Assistant,
            content,
        );
        let id = session.message_id.lock().clone();
        session.emitter.emit_with(
            AgentEventType::MessageCompleted,
            serde_json::to_value(MessageCompletedPayload { message }).unwrap_or_default(),
            None,
            id,
            None,
        );
    }

    if !session.shutting_down.load(Ordering::SeqCst) {
        let error = fleet_core_common::ApiError::new(
            "generic_process_exited",
            "the generic PTY process exited",
        );
        session.emitter.emit(
            AgentEventType::SessionError,
            serde_json::to_value(SessionErrorPayload { error }).unwrap_or_default(),
        );
        emit_status(&session.emitter, SessionStatus::Failed);
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

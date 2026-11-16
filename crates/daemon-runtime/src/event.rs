//! Event emission and per-session sequencing.
//!
//! Every provider adapter receives an [`AgentEventEmitter`] through
//! [`crate::ProviderContext`]. The emitter stamps each event with a
//! per-session, monotonically increasing sequence number and forwards it to the
//! runtime's broadcast event bus.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentPermissionRequest, AgentQuestionRequest, PermissionDecision,
    PermissionEventPayload, PermissionStatus, QuestionDecision, QuestionEventPayload,
    QuestionStatus, SessionStatus, StatusChangedPayload,
};
use fleet_core_protocol::common::{MessageId, SessionId, TurnId};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};

/// An event after the runtime has applied it and (for durable events)
/// recorded it in the daemon outbox. The cursor is the outbox sequence the
/// admin must acknowledge.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedAgentEvent {
    pub event: AgentEvent,
    pub cursor: Option<u64>,
}

/// A permission request that is waiting for a decision.
pub(crate) struct PendingPermission {
    pub request: AgentPermissionRequest,
    /// `None` when the request was emitted without going through
    /// [`AgentEventEmitter::request_permission`]; such requests can only be
    /// answered by the provider's native channel.
    pub sender: Option<oneshot::Sender<PermissionDecision>>,
}

/// Shared permission table for one session. The runtime and the emitter hold
/// the same allocation so a decision can be delivered even if event dispatch
/// has not caught up yet.
pub(crate) type PermissionMap = Arc<Mutex<std::collections::HashMap<String, PendingPermission>>>;

/// A user question that is waiting for a decision.
pub(crate) struct PendingQuestion {
    pub request: AgentQuestionRequest,
    /// `None` when the request was emitted without going through
    /// [`AgentEventEmitter::request_question`]; such requests can only be
    /// answered by the provider's native channel.
    pub sender: Option<oneshot::Sender<QuestionDecision>>,
}

pub(crate) type QuestionMap = Arc<Mutex<std::collections::HashMap<String, PendingQuestion>>>;

/// Per-session event emitter.
///
/// Cloning the emitter shares the sequence counter and the permission table;
/// clones therefore always produce a single ordered stream per session.
#[derive(Clone)]
pub struct AgentEventEmitter {
    session_id: SessionId,
    provider: String,
    sequence: Arc<AtomicU64>,
    tx: broadcast::Sender<AgentEvent>,
    /// Lossless dispatcher queue. Broadcast subscribers may lag (they recover
    /// from the outbox), but the runtime's state machine must observe every
    /// event exactly once and in order.
    dispatch: mpsc::UnboundedSender<AgentEvent>,
    permissions: PermissionMap,
    questions: QuestionMap,
    current_turn: Arc<Mutex<Option<TurnId>>>,
    /// Serializes sequence allocation and delivery so emitted events are
    /// observed in sequence order, and deduplicates status events.
    emit_lock: Arc<Mutex<()>>,
    last_status: Arc<Mutex<Option<SessionStatus>>>,
    /// Turn lifecycle events are singletons per turn. Runtime and providers may
    /// both observe the same transition; the marker prevents a duplicate
    /// `turn.started` / `turn.completed` from reaching the UI.
    turn_markers: Arc<Mutex<HashSet<String>>>,
}

impl AgentEventEmitter {
    pub(crate) fn new(
        session_id: impl Into<SessionId>,
        provider: impl Into<String>,
        tx: broadcast::Sender<AgentEvent>,
        dispatch: mpsc::UnboundedSender<AgentEvent>,
        permissions: PermissionMap,
        questions: QuestionMap,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            provider: provider.into(),
            sequence: Arc::new(AtomicU64::new(0)),
            tx,
            dispatch,
            permissions,
            questions,
            current_turn: Arc::new(Mutex::new(None)),
            emit_lock: Arc::new(Mutex::new(())),
            last_status: Arc::new(Mutex::new(None)),
            turn_markers: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Emit an event with no explicit turn/message/tool identifiers.
    ///
    /// The current turn (if any) is attached automatically.
    pub fn emit(&self, event_type: AgentEventType, payload: Value) {
        self.emit_with(event_type, payload, None, None, None);
    }

    /// Emit an event with explicit identifiers.
    ///
    /// Missing turn identifiers fall back to the session's current turn.
    pub fn emit_with(
        &self,
        event_type: AgentEventType,
        payload: Value,
        turn_id: Option<TurnId>,
        message_id: Option<MessageId>,
        tool_call_id: Option<String>,
    ) {
        let turn_id = turn_id.or_else(|| self.current_turn_id());
        if Self::is_turn_lifecycle(event_type) {
            if let Some(turn) = &turn_id {
                let key = format!("{turn}:{}", event_type.as_str());
                if !self.turn_markers.lock().insert(key) {
                    tracing::trace!(
                        session_id = %self.session_id,
                        event = event_type.as_str(),
                        "suppressing duplicate turn lifecycle event"
                    );
                    return;
                }
            }
        }

        let _guard = self.emit_lock.lock();
        if event_type == AgentEventType::AgentStatusChanged {
            let status = serde_json::from_value::<StatusChangedPayload>(payload.clone())
                .ok()
                .map(|payload| payload.status);
            if let Some(status) = status {
                let mut last = self.last_status.lock();
                if *last == Some(status) {
                    return;
                }
                *last = Some(status);
            }
        }
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let mut event = AgentEvent::new(
            self.session_id.clone(),
            sequence,
            event_type,
            self.provider.clone(),
        );
        event.turn_id = turn_id;
        event.message_id = message_id;
        event.tool_call_id = tool_call_id;
        event.payload = payload;

        // The dispatcher queue is lossless. A broadcast send can fail when
        // there are no subscribers; that is fine because durable events are
        // replayable from the outbox and ephemeral deltas are live-only.
        let _ = self.dispatch.send(event.clone());
        let _ = self.tx.send(event);
    }

    /// Session id this emitter belongs to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Provider id stamped on events.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The turn that subsequent events are attributed to, when known.
    pub fn current_turn_id(&self) -> Option<TurnId> {
        self.current_turn.lock().clone()
    }

    pub(crate) fn set_current_turn(&self, turn_id: Option<TurnId>) {
        *self.current_turn.lock() = turn_id;
    }

    /// Register a permission request and wait for its decision.
    ///
    /// The request is recorded in the shared permission table *before* the
    /// `permission.requested` event is emitted, so a caller that reacts to the
    /// event can resolve it immediately without a race.
    pub fn request_permission(
        &self,
        mut permission: AgentPermissionRequest,
    ) -> oneshot::Receiver<PermissionDecision> {
        if permission.session_id.is_empty() {
            permission.session_id = self.session_id.clone();
        }
        if permission.turn_id.is_none() {
            permission.turn_id = self.current_turn_id();
        }
        let turn_id = permission.turn_id.clone();
        let (tx, rx) = oneshot::channel();
        self.permissions.lock().insert(
            permission.id.clone(),
            PendingPermission {
                request: permission.clone(),
                sender: Some(tx),
            },
        );
        let payload = serde_json::to_value(PermissionEventPayload {
            permission: permission.clone(),
        })
        .unwrap_or(Value::Null);
        self.emit_with(
            AgentEventType::PermissionRequested,
            payload,
            turn_id,
            None,
            permission.tool_call_id,
        );
        rx
    }

    pub(crate) fn permissions(&self) -> &PermissionMap {
        &self.permissions
    }

    /// Resolve a pending permission without a user decision (timeout, cancel,
    /// interrupt). Emits a `permission.rejected` event with the terminal status
    /// so the runtime leaves `waiting_for_permission`.
    pub fn resolve_permission_locally(
        &self,
        permission_id: &str,
        status: PermissionStatus,
    ) -> Option<AgentPermissionRequest> {
        let permission = {
            let mut permissions = self.permissions.lock();
            let entry = permissions.get_mut(permission_id)?;
            if entry.request.status.is_terminal() {
                return None;
            }
            entry.request.status = status;
            entry.request.resolved_at = Some(fleet_core_common::now());
            entry.sender.take();
            entry.request.clone()
        };
        let payload = serde_json::to_value(PermissionEventPayload {
            permission: permission.clone(),
        })
        .unwrap_or(Value::Null);
        self.emit_with(
            AgentEventType::PermissionRejected,
            payload,
            permission.turn_id.clone(),
            None,
            permission.tool_call_id.clone(),
        );
        Some(permission)
    }

    /// Register a question request and wait for its decision.
    pub fn request_question(
        &self,
        mut question: AgentQuestionRequest,
    ) -> oneshot::Receiver<QuestionDecision> {
        if question.session_id.is_empty() {
            question.session_id = self.session_id.clone();
        }
        if question.turn_id.is_none() {
            question.turn_id = self.current_turn_id();
        }
        let turn_id = question.turn_id.clone();
        let (tx, rx) = oneshot::channel();
        self.questions.lock().insert(
            question.id.clone(),
            PendingQuestion {
                request: question.clone(),
                sender: Some(tx),
            },
        );
        let payload = serde_json::to_value(QuestionEventPayload {
            question: question.clone(),
        })
        .unwrap_or(Value::Null);
        self.emit_with(
            AgentEventType::QuestionRequested,
            payload,
            turn_id,
            None,
            question.tool_call_id,
        );
        rx
    }

    /// Resolve a pending question without a user decision (timeout, cancel,
    /// interrupt). Emits a `question.rejected` event with the terminal status.
    pub fn resolve_question_locally(
        &self,
        question_id: &str,
        status: QuestionStatus,
    ) -> Option<AgentQuestionRequest> {
        let question = {
            let mut questions = self.questions.lock();
            let entry = questions.get_mut(question_id)?;
            if entry.request.status.is_terminal() {
                return None;
            }
            entry.request.status = status;
            entry.request.resolved_at = Some(fleet_core_common::now());
            entry.sender.take();
            entry.request.clone()
        };
        let payload = serde_json::to_value(QuestionEventPayload {
            question: question.clone(),
        })
        .unwrap_or(Value::Null);
        self.emit_with(
            AgentEventType::QuestionRejected,
            payload,
            question.turn_id.clone(),
            None,
            question.tool_call_id.clone(),
        );
        Some(question)
    }

    fn is_turn_lifecycle(event_type: AgentEventType) -> bool {
        matches!(
            event_type,
            AgentEventType::TurnStarted
                | AgentEventType::TurnCompleted
                | AgentEventType::TurnInterrupted
                | AgentEventType::TurnFailed
        )
    }
}

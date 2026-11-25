//! Event emission and per-session sequencing.
//!
//! Every provider adapter receives an [`AgentEventEmitter`] through
//! [`crate::ProviderContext`]. The emitter stamps each event with a
//! per-session, monotonically increasing sequence number and forwards it to the
//! runtime's broadcast event bus.

pub mod dedup;

// `event` is private at the crate root, so the re-export is only reachable
// once the crate root wires it up; keep the public surface in place anyway.
#[allow(unused_imports)]
pub use dedup::EventDeduplicator;

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

/// Capacity of the state/persistence lane.
///
/// Events in this lane drive the in-memory state machine and the durable
/// outbox. They are never dropped: when the lane is full, new state events
/// move to the unbounded overflow lane, which is drained first on the next
/// dispatcher iteration.
const STATE_LANE_CAPACITY: usize = 4096;
/// Capacity of the live-only delta lane. Streaming deltas may be dropped
/// under sustained backpressure; the final `message.completed` remains
/// authoritative and is durable.
const DELTA_LANE_CAPACITY: usize = 2048;

/// Queues that feed the runtime event dispatcher.
///
/// The split keeps memory bounded without ever dropping a state transition:
/// high-rate ephemeral deltas have their own bounded lane (drop-on-full with
/// a counter), while durable state events always reach the dispatcher through
/// the state or overflow lane.
#[derive(Clone)]
pub struct DispatchQueues {
    state: mpsc::Sender<AgentEvent>,
    delta: mpsc::Sender<AgentEvent>,
    overflow: mpsc::UnboundedSender<AgentEvent>,
    /// Routing state shared by producers and the dispatcher.
    ///
    /// Taking this lock for the routing decision (and for clearing the flag)
    /// gives a total order: once the dispatcher observes an empty overflow
    /// lane and disables it, every later state event goes to the main lane,
    /// so state events can never be reordered across the two lanes.
    routing: Arc<Mutex<OverflowRouting>>,
    dropped_ephemeral: Arc<AtomicU64>,
    /// Durable events the store permanently rejected (constraint or
    /// invalid-data failures). These cannot be repaired by retrying.
    rejected_events: Arc<AtomicU64>,
    /// Events whose payload was truncated to fit the transport limit.
    truncated_events: Arc<AtomicU64>,
}

#[derive(Default)]
struct OverflowRouting {
    /// Whether producers must use the overflow lane.
    active: bool,
    /// Events queued in the overflow lane but not yet consumed.
    len: usize,
}

/// Receiving halves owned by the dispatcher task.
pub struct DispatchReceivers {
    pub state: mpsc::Receiver<AgentEvent>,
    pub delta: mpsc::Receiver<AgentEvent>,
    pub overflow: mpsc::UnboundedReceiver<AgentEvent>,
}

impl DispatchQueues {
    /// Create the queues and their receiving halves.
    pub fn new() -> (Self, DispatchReceivers) {
        let (state, state_rx) = mpsc::channel(STATE_LANE_CAPACITY);
        let (delta, delta_rx) = mpsc::channel(DELTA_LANE_CAPACITY);
        let (overflow, overflow_rx) = mpsc::unbounded_channel();
        (
            Self {
                state,
                delta,
                overflow,
                routing: Arc::new(Mutex::new(OverflowRouting::default())),
                dropped_ephemeral: Arc::new(AtomicU64::new(0)),
                rejected_events: Arc::new(AtomicU64::new(0)),
                truncated_events: Arc::new(AtomicU64::new(0)),
            },
            DispatchReceivers {
                state: state_rx,
                delta: delta_rx,
                overflow: overflow_rx,
            },
        )
    }

    /// Route one event to the appropriate lane.
    pub(crate) fn send(&self, event: AgentEvent) {
        if droppable_ephemeral(&event) {
            if self.delta.try_send(event).is_err() {
                let dropped = self.dropped_ephemeral.fetch_add(1, Ordering::Relaxed) + 1;
                if dropped == 1 || dropped.is_multiple_of(1_000) {
                    tracing::warn!(
                        dropped,
                        "dropping ephemeral agent events under event-queue backpressure"
                    );
                }
            }
            return;
        }
        let mut routing = self.routing.lock();
        if !routing.active {
            match self.state.try_send(event) {
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(event)) => {
                    routing.active = true;
                    Self::push_overflow_locked(&self.overflow, &mut routing, event);
                    return;
                }
            }
        }
        Self::push_overflow_locked(&self.overflow, &mut routing, event);
    }

    fn push_overflow_locked(
        overflow: &mpsc::UnboundedSender<AgentEvent>,
        routing: &mut OverflowRouting,
        event: AgentEvent,
    ) {
        routing.len += 1;
        if overflow.send(event).is_err() {
            routing.len -= 1;
        }
    }

    /// Number of state-lane events not yet consumed.
    pub fn state_depth(&self) -> usize {
        self.state
            .max_capacity()
            .saturating_sub(self.state.capacity())
    }

    /// Number of delta-lane events not yet consumed.
    pub fn delta_depth(&self) -> usize {
        self.delta
            .max_capacity()
            .saturating_sub(self.delta.capacity())
    }

    /// Number of state events waiting in the overflow lane.
    pub fn overflow_depth(&self) -> usize {
        self.routing.lock().len
    }

    /// Ephemeral deltas dropped because the delta lane stayed full.
    pub fn dropped_ephemeral(&self) -> u64 {
        self.dropped_ephemeral.load(Ordering::Relaxed)
    }

    /// Durable events permanently rejected by the store.
    pub fn rejected_events(&self) -> u64 {
        self.rejected_events.load(Ordering::Relaxed)
    }

    /// Account for one durable event the store permanently rejected.
    pub(crate) fn note_store_rejection(&self) {
        self.rejected_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Account for one event whose payload was truncated for transport.
    pub(crate) fn note_truncated_event(&self) {
        self.truncated_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Events truncated because their payload exceeded the transport limit.
    pub fn truncated_events(&self) -> u64 {
        self.truncated_events.load(Ordering::Relaxed)
    }

    /// Disable the overflow lane when it is empty.
    ///
    /// Returns `true` once producers are routed back to the main lane. A
    /// producer that queued an event before this call keeps the lane active
    /// (`false`), so the dispatcher drains it first and state events stay
    /// ordered.
    pub(crate) fn try_end_overflow(&self) -> bool {
        let mut routing = self.routing.lock();
        if routing.len == 0 {
            routing.active = false;
            true
        } else {
            false
        }
    }

    /// Account for one consumed overflow event.
    pub(crate) fn overflow_received(&self) {
        let mut routing = self.routing.lock();
        routing.len = routing.len.saturating_sub(1);
    }

    /// Account for one ephemeral event discarded after it was queued (for
    /// example a delta that arrived after its message completed).
    pub(crate) fn note_dropped_ephemeral(&self) {
        self.dropped_ephemeral.fetch_add(1, Ordering::Relaxed);
    }
}

/// Events the dispatcher does not apply to state.
///
/// Dropping these under backpressure only loses live streaming text; the
/// authoritative message is delivered by `message.completed`. Raw events that
/// carry the provider session id are routed as state events because the
/// runtime persists them.
fn droppable_ephemeral(event: &AgentEvent) -> bool {
    match event.event_type {
        AgentEventType::MessageDelta
        | AgentEventType::ToolInputDelta
        | AgentEventType::TerminalOutput => true,
        AgentEventType::AgentRawEvent => {
            let provider_id = event
                .payload
                .get("provider_session_id")
                .and_then(Value::as_str)
                .or_else(|| {
                    event
                        .raw_event
                        .as_ref()
                        .and_then(|raw| raw.get("provider_session_id"))
                        .and_then(Value::as_str)
                });
            provider_id.is_none()
        }
        _ => false,
    }
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
    /// Dispatcher queues. Broadcast subscribers may lag (they recover from
    /// the outbox), but the runtime's state machine must observe every state
    /// event exactly once and in order. High-rate deltas have a separate
    /// bounded lane so memory stays bounded under backpressure.
    dispatch: DispatchQueues,
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
        dispatch: DispatchQueues,
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

        // State events are never dropped by the dispatcher queues; only
        // ephemeral deltas may be shed under backpressure. A broadcast send
        // can fail when there are no subscribers; that is fine because
        // durable events are replayable from the outbox.
        self.dispatch.send(event.clone());
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

    /// Latest per-session event sequence. Used as a cheap activity signal.
    pub fn event_sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    pub(crate) fn set_current_turn(&self, turn_id: Option<TurnId>) {
        *self.current_turn.lock() = turn_id;
    }

    /// Subscribe to the emitter's raw event bus (unit tests only).
    #[cfg(test)]
    pub(crate) fn subscribe_for_test(&self) -> broadcast::Receiver<AgentEvent> {
        self.tx.subscribe()
    }

    /// Set the current turn from a unit test.
    #[cfg(test)]
    pub(crate) fn set_current_turn_pub(&self, turn_id: Option<TurnId>) {
        self.set_current_turn(turn_id);
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

    /// Mark an already-decided question as not delivered to the provider.
    /// Used when the native reply itself fails after the runtime resolved the
    /// oneshot.
    pub fn fail_question_delivery(&self, question_id: &str) -> Option<AgentQuestionRequest> {
        let question = {
            let mut questions = self.questions.lock();
            let entry = questions.get_mut(question_id)?;
            entry.request.status = QuestionStatus::DeliveryFailed;
            entry.request.resolved_at = Some(fleet_core_common::now());
            entry.sender.take();
            entry.request.clone()
        };
        self.emit_with(
            AgentEventType::QuestionRejected,
            serde_json::to_value(QuestionEventPayload {
                question: question.clone(),
            })
            .unwrap_or(Value::Null),
            question.turn_id.clone(),
            None,
            question.tool_call_id.clone(),
        );
        Some(question)
    }

    /// Resolve every pending question, for example when a provider process
    /// dies while a question is open.
    pub fn resolve_all_pending_questions(&self, status: QuestionStatus) -> usize {
        let ids: Vec<String> = self
            .questions
            .lock()
            .iter()
            .filter(|(_, entry)| entry.request.status == QuestionStatus::Pending)
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter()
            .filter(|id| self.resolve_question_locally(id, status).is_some())
            .count()
    }

    /// Resolve every pending permission for provider lifecycle failures.
    pub fn resolve_all_pending_permissions(&self, status: PermissionStatus) -> usize {
        let ids: Vec<String> = self
            .permissions
            .lock()
            .iter()
            .filter(|(_, entry)| entry.request.status == PermissionStatus::Pending)
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter()
            .filter(|id| self.resolve_permission_locally(id, status).is_some())
            .count()
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

#[cfg(test)]
mod queue_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_full_state_lane_moves_events_to_the_overflow_lane() {
        let (queues, mut receivers) = DispatchQueues::new();
        for sequence in 1..=(STATE_LANE_CAPACITY as u64 + 1) {
            queues.send(AgentEvent::new(
                "s1",
                sequence,
                AgentEventType::TurnStarted,
                "mock",
            ));
        }
        let mut received = 0;
        while receivers.state.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, STATE_LANE_CAPACITY);
        let overflow = receivers
            .overflow
            .try_recv()
            .expect("the overflowing state event must be retained");
        assert_eq!(overflow.sequence, STATE_LANE_CAPACITY as u64 + 1);
        assert_eq!(queues.overflow_depth(), 1);
        assert_eq!(queues.dropped_ephemeral(), 0);
    }

    #[test]
    fn a_full_delta_lane_drops_only_ephemeral_events() {
        let (queues, mut receivers) = DispatchQueues::new();
        for _ in 0..DELTA_LANE_CAPACITY {
            queues.send(AgentEvent::new(
                "s1",
                1,
                AgentEventType::MessageDelta,
                "mock",
            ));
        }
        queues.send(AgentEvent::new(
            "s1",
            2,
            AgentEventType::MessageDelta,
            "mock",
        ));
        assert_eq!(queues.dropped_ephemeral(), 1);
        assert_eq!(receivers.delta.try_recv().unwrap().sequence, 1);
        assert!(receivers.state.try_recv().is_err());
    }

    #[test]
    fn raw_events_carrying_a_provider_session_id_are_state_events() {
        let (queues, mut receivers) = DispatchQueues::new();
        queues.send(
            AgentEvent::new("s1", 1, AgentEventType::AgentRawEvent, "mock")
                .with_payload(&json!({ "provider_session_id": "native_1" })),
        );
        assert!(receivers.state.try_recv().is_ok());
        assert_eq!(queues.dropped_ephemeral(), 0);

        queues.send(
            AgentEvent::new("s1", 2, AgentEventType::AgentRawEvent, "mock")
                .with_payload(&json!({ "kind": "thought_chunk" })),
        );
        assert!(receivers.delta.try_recv().is_ok());
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;

    #[test]
    fn the_overflow_lane_closes_only_when_empty() {
        let (queues, mut receivers) = DispatchQueues::new();
        for sequence in 1..=(STATE_LANE_CAPACITY as u64 + 1) {
            queues.send(AgentEvent::new(
                "s1",
                sequence,
                AgentEventType::TurnStarted,
                "mock",
            ));
        }
        while receivers.state.try_recv().is_ok() {}
        let overflowed = receivers.overflow.try_recv().expect("overflow event");
        assert_eq!(overflowed.sequence, STATE_LANE_CAPACITY as u64 + 1);

        // The receiver is drained but the accounting still shows the event
        // until the dispatcher reports it.
        assert!(!queues.try_end_overflow());
        queues.overflow_received();
        assert!(queues.try_end_overflow());

        // New state events use the main lane again.
        queues.send(AgentEvent::new(
            "s1",
            STATE_LANE_CAPACITY as u64 + 2,
            AgentEventType::TurnCompleted,
            "mock",
        ));
        assert!(receivers.state.try_recv().is_ok());
        assert!(receivers.overflow.try_recv().is_err());
    }
}

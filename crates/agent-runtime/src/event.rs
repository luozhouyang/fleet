//! Event emission and per-session sequencing.
//!
//! Every provider adapter receives an [`AgentEventEmitter`] through
//! [`crate::ProviderContext`]. The emitter stamps each event with a
//! per-session, monotonically increasing sequence number and forwards it to the
//! runtime's broadcast event bus.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_fleet_protocol::agent::{
    AgentEvent, AgentEventType, AgentPermissionRequest, PermissionDecision, PermissionEventPayload,
    PermissionStatus, SessionStatus, StatusChangedPayload,
};
use agent_fleet_protocol::common::{MessageId, SessionId, TurnId};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};

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
    permissions: PermissionMap,
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
        permissions: PermissionMap,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            provider: provider.into(),
            sequence: Arc::new(AtomicU64::new(0)),
            tx,
            permissions,
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

        // A broadcast send fails when there are no subscribers. Events are
        // best-effort: the runtime records state through the same bus, but a
        // detached session is allowed to have no listeners.
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
            permission.tool_call_id.clone(),
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
            entry.request.resolved_at = Some(agent_fleet_common::now());
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

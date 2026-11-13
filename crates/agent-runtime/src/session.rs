//! Runtime-owned session state.

use agent_fleet_common::now;
use agent_fleet_protocol::agent::{
    AgentMessage, AgentSession, AgentTurn, SessionStatus, TurnStatus,
};
use agent_fleet_protocol::common::TurnId;
use parking_lot::Mutex;
use tokio::sync::RwLock;

use crate::cancel::CancelFlag;
use crate::event::{AgentEventEmitter, PermissionMap};

/// A turn that has started but has not reached a terminal state yet.
#[derive(Debug, Clone)]
pub(crate) struct InFlightTurn {
    pub turn_id: TurnId,
}

/// Mutable state the runtime keeps for one session.
pub(crate) struct SessionHandle {
    pub session: RwLock<AgentSession>,
    pub emitter: AgentEventEmitter,
    pub turns: Mutex<Vec<AgentTurn>>,
    pub messages: Mutex<Vec<AgentMessage>>,
    pub permissions: PermissionMap,
    pub in_flight: Mutex<Vec<InFlightTurn>>,
    pub cancel: CancelFlag,
}

impl SessionHandle {
    pub(crate) fn new(
        session: AgentSession,
        emitter: AgentEventEmitter,
        permissions: PermissionMap,
    ) -> Self {
        Self {
            session: RwLock::new(session),
            emitter,
            turns: Mutex::new(Vec::new()),
            messages: Mutex::new(Vec::new()),
            permissions,
            in_flight: Mutex::new(Vec::new()),
            cancel: CancelFlag::new(),
        }
    }

    /// Record a turn that is starting. Replaces an existing record with the
    /// same id.
    pub(crate) fn upsert_turn(&self, turn: AgentTurn) {
        let mut turns = self.turns.lock();
        if let Some(existing) = turns.iter_mut().find(|item| item.id == turn.id) {
            *existing = turn;
        } else {
            turns.push(turn);
        }
    }

    /// Mark a turn terminal.
    ///
    /// When `turn_id` is absent the most recently started in-flight turn is
    /// used. Returns the updated turn when one was found.
    pub(crate) fn finish_turn(
        &self,
        turn_id: Option<&str>,
        status: TurnStatus,
        error: Option<agent_fleet_common::ApiError>,
    ) -> Option<AgentTurn> {
        let target = turn_id.map(str::to_string).or_else(|| {
            self.in_flight
                .lock()
                .last()
                .map(|in_flight| in_flight.turn_id.clone())
        })?;

        let updated = {
            let mut turns = self.turns.lock();
            let turn = turns.iter_mut().find(|item| item.id == target)?;
            if turn.status.is_terminal() {
                return None;
            }
            turn.status = status;
            turn.completed_at = Some(now());
            turn.error = error;
            turn.clone()
        };

        self.in_flight
            .lock()
            .retain(|in_flight| in_flight.turn_id != target);
        if self.emitter.current_turn_id().as_deref() == Some(target.as_str()) {
            self.emitter.set_current_turn(None);
        }
        Some(updated)
    }

    /// Append a message unless a message with the same id is already recorded.
    pub(crate) fn record_message(&self, message: AgentMessage) {
        let mut messages = self.messages.lock();
        if !messages.iter().any(|existing| existing.id == message.id) {
            messages.push(message);
        }
    }

    /// Record a newly started turn and its assistant message slot.
    pub(crate) fn begin_turn(&self, turn: AgentTurn) {
        self.emitter.set_current_turn(Some(turn.id.clone()));
        self.in_flight.lock().push(InFlightTurn {
            turn_id: turn.id.clone(),
        });
        self.turns.lock().push(turn);
    }

    /// Look up the current session status.
    pub(crate) async fn status(&self) -> SessionStatus {
        self.session.read().await.status
    }
}

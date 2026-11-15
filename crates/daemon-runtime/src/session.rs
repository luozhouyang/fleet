//! Runtime-owned session state.

use fleet_core_common::now;
use fleet_core_protocol::agent::{
    AgentMessage, AgentSession, AgentToolCall, AgentTurn, SessionStatus, ToolCallStatus, TurnStatus,
};
use fleet_core_protocol::common::TurnId;
use parking_lot::Mutex;
use tokio::sync::RwLock;

use crate::cancel::CancelFlag;
use crate::event::{AgentEventEmitter, PermissionMap};

/// Keep the live view bounded; the database retains the full transcript.
const MAX_IN_MEMORY_MESSAGES: usize = 5_000;
/// Keep the live view bounded; the database retains full turn history.
const MAX_IN_MEMORY_TURNS: usize = 2_000;
/// Keep the live view bounded; the database retains full tool history.
const MAX_IN_MEMORY_TOOLS: usize = 5_000;

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
    pub tools: Mutex<Vec<AgentToolCall>>,
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
            tools: Mutex::new(Vec::new()),
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
        trim_front(&mut turns, MAX_IN_MEMORY_TURNS);
    }

    /// Mark a turn terminal.
    ///
    /// When `turn_id` is absent the most recently started in-flight turn is
    /// used. Returns the updated turn when one was found.
    pub(crate) fn finish_turn(
        &self,
        turn_id: Option<&str>,
        status: TurnStatus,
        error: Option<fleet_core_common::ApiError>,
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
            trim_front(&mut messages, MAX_IN_MEMORY_MESSAGES);
        }
    }

    /// Record or replace one structured tool call.
    pub(crate) fn upsert_tool(&self, tool: AgentToolCall) {
        let mut tools = self.tools.lock();
        if let Some(existing) = tools.iter_mut().find(|item| item.id == tool.id) {
            *existing = tool;
        } else {
            tools.push(tool);
        }
        trim_front(&mut tools, MAX_IN_MEMORY_TOOLS);
    }

    /// Mark a known tool call terminal. Returns the updated record.
    pub(crate) fn finish_tool(
        &self,
        tool_call_id: &str,
        status: ToolCallStatus,
        error: Option<String>,
    ) -> Option<AgentToolCall> {
        let mut tools = self.tools.lock();
        let tool = tools.iter_mut().find(|item| item.id == tool_call_id)?;
        if tool.status == status && tool.error == error {
            return Some(tool.clone());
        }
        tool.status = status;
        tool.error = error;
        tool.completed_at = Some(now());
        Some(tool.clone())
    }

    /// Record a newly started turn and its assistant message slot.
    pub(crate) fn begin_turn(&self, turn: AgentTurn) {
        self.emitter.set_current_turn(Some(turn.id.clone()));
        self.in_flight.lock().push(InFlightTurn {
            turn_id: turn.id.clone(),
        });
        let mut turns = self.turns.lock();
        turns.push(turn);
        trim_front(&mut turns, MAX_IN_MEMORY_TURNS);
    }

    /// Trim an externally loaded transcript to the in-memory bound.
    pub(crate) fn trim_history(&self) {
        trim_front(&mut self.turns.lock(), MAX_IN_MEMORY_TURNS);
        trim_front(&mut self.messages.lock(), MAX_IN_MEMORY_MESSAGES);
        trim_front(&mut self.tools.lock(), MAX_IN_MEMORY_TOOLS);
    }

    /// Look up the current session status.
    pub(crate) async fn status(&self) -> SessionStatus {
        self.session.read().await.status
    }
}

fn trim_front<T>(items: &mut Vec<T>, limit: usize) {
    if items.len() > limit {
        let excess = items.len() - limit;
        items.drain(..excess);
    }
}

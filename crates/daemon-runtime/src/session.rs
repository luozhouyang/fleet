//! Runtime-owned session state.

use std::sync::atomic::{AtomicU64, Ordering};

use fleet_core_common::now;
use fleet_core_protocol::agent::{
    AgentMessage, AgentSession, AgentToolCall, AgentTurn, SessionStatus, ToolCallStatus, TurnStatus,
};
use fleet_core_protocol::common::TurnId;
use parking_lot::Mutex;
use tokio::sync::RwLock;

use crate::cancel::CancelFlag;
use crate::error::RuntimeError;
use crate::event::{AgentEventEmitter, PermissionMap, QuestionMap};

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
    pub questions: QuestionMap,
    pub in_flight: Mutex<Vec<InFlightTurn>>,
    pub cancel: CancelFlag,
    /// Monotonic interrupt counter. `send_message` captures it before the
    /// driver dispatch so a concurrent interrupt that may have been
    /// delivered first can cancel the late prompt again.
    interrupt_epoch: AtomicU64,
}

impl SessionHandle {
    pub(crate) fn new(
        session: AgentSession,
        emitter: AgentEventEmitter,
        permissions: PermissionMap,
        questions: QuestionMap,
    ) -> Self {
        Self {
            session: RwLock::new(session),
            emitter,
            turns: Mutex::new(Vec::new()),
            messages: Mutex::new(Vec::new()),
            tools: Mutex::new(Vec::new()),
            permissions,
            questions,
            in_flight: Mutex::new(Vec::new()),
            cancel: CancelFlag::new(),
            interrupt_epoch: AtomicU64::new(0),
        }
    }

    /// Current interrupt generation.
    pub(crate) fn interrupt_epoch(&self) -> u64 {
        self.interrupt_epoch.load(Ordering::SeqCst)
    }

    /// Record that an interrupt was requested.
    pub(crate) fn request_interrupt(&self) -> u64 {
        self.interrupt_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Record a turn that is starting. Replaces an existing record with the
    /// same id unless that would reopen a terminal turn.
    pub(crate) fn upsert_turn(&self, turn: AgentTurn) {
        let mut turns = self.turns.lock();
        if let Some(existing) = turns.iter_mut().find(|item| item.id == turn.id) {
            // A late `turn.started` from a replay must not overwrite a turn
            // that already reached a terminal state.
            if existing.status.is_terminal() && !turn.status.is_terminal() {
                return;
            }
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
    ///
    /// Providers often send a completion with only the id and output. Keep the
    /// name, input and start time learned from the start event instead of
    /// overwriting them with placeholders.
    pub(crate) fn upsert_tool(&self, mut tool: AgentToolCall) -> AgentToolCall {
        let mut tools = self.tools.lock();
        let stored = if let Some(existing) = tools.iter_mut().find(|item| item.id == tool.id) {
            merge_tool(&mut tool, existing);
            *existing = tool;
            existing.clone()
        } else {
            tools.push(tool.clone());
            tool
        };
        trim_front(&mut tools, MAX_IN_MEMORY_TOOLS);
        stored
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

    /// Atomically admit a user-prompt turn.
    ///
    /// The in-flight check and the insertion happen under one lock, so two
    /// concurrent `send_message` calls can never both start a turn for the
    /// same session. The turn id also becomes the emitter's current turn in
    /// the same critical section, keeping event correlation consistent with
    /// the admitted turn.
    pub(crate) fn try_begin_turn(&self, turn: AgentTurn) -> Result<(), RuntimeError> {
        let mut in_flight = self.in_flight.lock();
        if !in_flight.is_empty() {
            return Err(RuntimeError::InvalidState(
                "a turn is already in flight for this session".to_string(),
            ));
        }
        self.emitter.set_current_turn(Some(turn.id.clone()));
        in_flight.push(InFlightTurn {
            turn_id: turn.id.clone(),
        });
        let mut turns = self.turns.lock();
        turns.push(turn);
        trim_front(&mut turns, MAX_IN_MEMORY_TURNS);
        Ok(())
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

fn merge_tool(incoming: &mut AgentToolCall, existing: &AgentToolCall) {
    if incoming.name.trim().is_empty() || incoming.name == "tool" {
        incoming.name = existing.name.clone();
    }
    if incoming.input.is_null() {
        incoming.input = existing.input.clone();
    }
    if incoming.turn_id.is_none() {
        incoming.turn_id = existing.turn_id.clone();
    }
    if incoming.output.is_null() {
        incoming.output = existing.output.clone();
    }
    if incoming.error.is_none() {
        incoming.error = existing.error.clone();
    }
    if incoming.started_at > existing.started_at {
        incoming.started_at = existing.started_at;
    }
    if incoming.completed_at.is_none() {
        incoming.completed_at = existing.completed_at;
    }
}

fn trim_front<T>(items: &mut Vec<T>, limit: usize) {
    if items.len() > limit {
        let excess = items.len() - limit;
        items.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Barrier};

    use fleet_core_protocol::agent::AgentSession;
    use tokio::sync::broadcast;

    use super::*;

    fn test_handle() -> Arc<SessionHandle> {
        let (events, _) = broadcast::channel(64);
        let (dispatch, _receivers) = crate::event::DispatchQueues::new();
        let permissions = Arc::new(Mutex::new(HashMap::new()));
        let questions = Arc::new(Mutex::new(HashMap::new()));
        let emitter = AgentEventEmitter::new(
            "session_1",
            "mock",
            events,
            dispatch,
            Arc::clone(&permissions),
            Arc::clone(&questions),
        );
        Arc::new(SessionHandle::new(
            AgentSession::new("machine_1", "mock", "mock", "/tmp"),
            emitter,
            permissions,
            questions,
        ))
    }

    fn running_turn() -> AgentTurn {
        let mut turn = AgentTurn::new("session_1");
        turn.status = TurnStatus::Running;
        turn
    }

    #[test]
    fn concurrent_turn_admission_allows_exactly_one_turn() {
        let handle = test_handle();
        let barrier = Arc::new(Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let handle = Arc::clone(&handle);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                handle.try_begin_turn(running_turn()).is_ok()
            }));
        }
        let admitted = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker"))
            .filter(|admitted| *admitted)
            .count();
        assert_eq!(admitted, 1, "at most one turn may be in flight");
        assert_eq!(handle.in_flight.lock().len(), 1);
    }

    #[test]
    fn a_finished_turn_frees_admission() {
        let handle = test_handle();
        let first = running_turn();
        handle.try_begin_turn(first.clone()).unwrap();
        assert!(handle.try_begin_turn(running_turn()).is_err());

        let finished = handle
            .finish_turn(Some(first.id.as_str()), TurnStatus::Completed, None)
            .expect("turn finished");
        assert_eq!(finished.id, first.id);
        assert!(handle.try_begin_turn(running_turn()).is_ok());
        assert_eq!(handle.in_flight.lock().len(), 1);
    }

    #[test]
    fn a_late_start_does_not_reopen_a_terminal_turn() {
        let handle = test_handle();
        let turn = running_turn();
        handle.try_begin_turn(turn.clone()).unwrap();
        handle
            .finish_turn(Some(turn.id.as_str()), TurnStatus::Completed, None)
            .unwrap();

        let mut late = turn.clone();
        late.status = TurnStatus::Running;
        handle.upsert_turn(late);
        let stored = handle
            .turns
            .lock()
            .iter()
            .find(|item| item.id == turn.id)
            .cloned()
            .expect("turn");
        assert_eq!(stored.status, TurnStatus::Completed);
    }
}

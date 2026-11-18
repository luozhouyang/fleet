//! Context passed to a driver when it starts an agent.

use fleet_core_protocol::agent::AgentSession;

use crate::event::AgentEventEmitter;

/// Everything a driver needs to start one agent instance.
///
/// The runtime owns the Fleet session and the event bus; the driver owns the
/// protocol connection and any child process.
pub struct DriverContext {
    /// Machine that owns the session.
    pub machine_id: String,
    /// Instance id allocated by the runtime before the agent is started.
    pub instance_id: String,
    /// Session snapshot at start time (status `Starting`).
    pub session: AgentSession,
    /// Per-session event sink. Clones share the session's sequence counter.
    pub emitter: AgentEventEmitter,
}

impl DriverContext {
    /// Fleet session id for this instance.
    pub fn fleet_session_id(&self) -> &str {
        &self.session.id
    }
}

impl std::fmt::Debug for DriverContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverContext")
            .field("machine_id", &self.machine_id)
            .field("instance_id", &self.instance_id)
            .field("fleet_session_id", &self.session.id)
            .finish_non_exhaustive()
    }
}

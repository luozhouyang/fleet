//! The identity of a running agent instance.

use fleet_core_protocol::agent::AgentSession;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A cheap, cloneable reference to one running agent instance.
///
/// The handle deliberately does not own the process or the connection. Those
/// live in the driver's per-instance state; the handle carries only stable
/// identifiers and non-sensitive diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInstanceHandle {
    /// Instance id allocated before the driver starts the agent.
    pub instance_id: String,
    /// Agent definition that produced this instance.
    pub definition_id: String,
    /// Driver that owns the instance.
    pub driver_id: String,
    /// Fleet session served by this instance.
    pub session_id: String,
    /// Operating-system process id, when the driver spawned a process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    /// Non-sensitive driver diagnostics.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

/// What a driver returns after starting an agent.
#[derive(Debug, Clone)]
pub struct DriverStartResult {
    /// Session updated with agent-reported values (metadata, agent type).
    pub session: AgentSession,
    /// Operating-system process id, when known.
    pub process_id: Option<u32>,
    /// Non-sensitive driver metadata merged into the session metadata.
    pub metadata: Value,
}

impl DriverStartResult {
    /// Build a result for a driver without a process.
    pub fn new(session: AgentSession) -> Self {
        Self {
            session,
            process_id: None,
            metadata: Value::Null,
        }
    }
}

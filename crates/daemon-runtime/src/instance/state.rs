//! Instance state model.

use chrono::{DateTime, Utc};
use fleet_core_common::now;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lifecycle of one agent instance.
///
/// This is a different state machine from session status: an instance can be
/// `Stopped` while its session is `Completed`, and vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    /// The process or connection is being created.
    Starting,
    /// The agent completed its handshake and can accept prompts.
    Ready,
    /// The agent has foreground work in progress.
    Busy,
    /// The daemon asked the agent to stop.
    Stopping,
    /// The agent stopped cleanly.
    Stopped,
    /// The agent failed to start or terminated with an error.
    Failed,
    /// The daemon lost track of the agent (for example after a daemon restart).
    Lost,
}

impl InstanceStatus {
    /// Whether no further transitions are expected for this instance.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Failed | Self::Lost)
    }

    /// Stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Busy => "busy",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Lost => "lost",
        }
    }
}

impl std::fmt::Display for InstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A running (or recently stopped) agent instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInstance {
    /// Instance id.
    pub id: String,
    /// Definition that produced the instance.
    pub definition_id: String,
    /// Driver that owns the instance.
    pub driver_id: String,
    /// Fleet session served by the instance.
    pub session_id: String,
    /// Operating-system process id, when the driver spawned a process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    /// Lifecycle status.
    pub status: InstanceStatus,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Time the agent became ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// Time the instance reached a terminal status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<DateTime<Utc>>,
    /// Non-sensitive driver metadata.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl AgentInstance {
    /// Create an instance in the `Starting` state.
    pub fn starting(
        id: impl Into<String>,
        definition_id: impl Into<String>,
        driver_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            definition_id: definition_id.into(),
            driver_id: driver_id.into(),
            session_id: session_id.into(),
            process_id: None,
            status: InstanceStatus::Starting,
            created_at: now(),
            started_at: None,
            stopped_at: None,
            metadata: Value::Null,
        }
    }

    /// Mark the instance ready.
    pub fn mark_ready(&mut self, process_id: Option<u32>, metadata: Value) {
        self.status = InstanceStatus::Ready;
        self.process_id = process_id;
        if !metadata.is_null() {
            self.metadata = metadata;
        }
        self.started_at = Some(now());
    }

    /// Return a busy instance to `Ready` after its foreground work finishes.
    ///
    /// Terminal states are never revived; only `Busy` transitions.
    pub fn mark_idle(&mut self) {
        if self.status == InstanceStatus::Busy {
            self.status = InstanceStatus::Ready;
        }
    }

    /// Mark the instance failed.
    pub fn mark_failed(&mut self, metadata: Value) {
        self.status = InstanceStatus::Failed;
        if !metadata.is_null() {
            self.metadata = metadata;
        }
        self.stopped_at = Some(now());
    }

    /// Mark the instance stopped.
    pub fn mark_stopped(&mut self) {
        self.status = InstanceStatus::Stopped;
        self.stopped_at = Some(now());
    }
}

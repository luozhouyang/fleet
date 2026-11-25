use serde::{Deserialize, Serialize};

use crate::common::{EventId, MachineId, SessionId, Timestamp, WorkspaceId};

/// Platform-level event forwarded by the agent admin to browsers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformEvent {
    pub event_id: EventId,
    /// Monotonic sequence per machine connection, used for gap detection.
    #[serde(default)]
    pub sequence: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub timestamp: Timestamp,
    /// Organization resolved when the event was published.
    ///
    /// The browser socket scopes delivery to this value so a ticket bound to
    /// one organization never receives another organization's events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<MachineId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl PlatformEvent {
    pub fn new(event_type: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            event_id: crate::common::new_id("evt"),
            sequence: 0,
            event_type: event_type.into(),
            timestamp: crate::common::now(),
            org_id: None,
            machine_id: None,
            workspace_id: None,
            session_id: None,
            payload,
        }
    }

    pub fn key(&self) -> &str {
        &self.event_type
    }
}

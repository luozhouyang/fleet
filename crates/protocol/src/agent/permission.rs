use serde::{Deserialize, Serialize};

use crate::common::{SessionId, Timestamp, TurnId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Pending,
    Approved,
    Rejected,
    TimedOut,
    Cancelled,
    DeliveryFailed,
}

impl PermissionStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// A request from the agent that requires a human decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPermissionRequest {
    pub id: String,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub description: String,
    #[serde(default)]
    pub input: serde_json::Value,
    pub status: PermissionStatus,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// The answer sent back to a provider adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionDecision {
    pub permission_id: String,
    pub approve: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Request payload for `agent.permissions.respond`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RespondPermissionRequest {
    pub permission_id: String,
    pub approve: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

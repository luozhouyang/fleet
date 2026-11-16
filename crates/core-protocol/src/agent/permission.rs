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

    /// Stable wire/database representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::DeliveryFailed => "delivery_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
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
    #[serde(default)]
    pub options: Vec<AgentPermissionOption>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub approve: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Request payload for `agent.permissions.respond`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RespondPermissionRequest {
    pub permission_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub approve: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::now;

    #[test]
    fn permission_option_round_trips() {
        let request = AgentPermissionRequest {
            id: "perm_1".into(),
            session_id: "session_1".into(),
            turn_id: None,
            tool_call_id: Some("call_1".into()),
            tool_name: "command_execution".into(),
            description: "Allow command?".into(),
            input: serde_json::json!({"command": "rm -rf /tmp/x"}),
            options: vec![AgentPermissionOption {
                option_id: "allow_once".into(),
                name: "Allow once".into(),
                kind: "allow_once".into(),
            }],
            status: PermissionStatus::Pending,
            created_at: now(),
            resolved_at: None,
            expires_at: None,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["options"][0]["option_id"], "allow_once");
        let decoded: AgentPermissionRequest = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn permission_decision_carries_scope_and_option() {
        let decision = PermissionDecision {
            permission_id: "perm_1".into(),
            session_id: Some("session_1".into()),
            approve: true,
            option_id: Some("allow_always".into()),
            message: None,
        };
        let value = serde_json::to_value(decision).unwrap();
        assert_eq!(value["session_id"], "session_1");
        assert_eq!(value["option_id"], "allow_always");
    }
}

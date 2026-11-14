use serde::{Deserialize, Serialize};

use crate::common::{ApiError, MachineId, SessionId, Timestamp, WorkspaceId};

/// Lifecycle of an agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Creating,
    Starting,
    Running,
    WaitingForInput,
    WaitingForPermission,
    Interrupted,
    Completed,
    Failed,
    Terminated,
}

impl SessionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Terminated)
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Creating
                | Self::Starting
                | Self::Running
                | Self::WaitingForInput
                | Self::WaitingForPermission
                | Self::Interrupted
        )
    }

    /// Guard used by the runtime before persisting a status transition.
    pub fn can_transition_to(self, next: SessionStatus) -> bool {
        use SessionStatus::*;
        if self == next {
            return true;
        }
        match self {
            Creating => matches!(next, Starting | Failed | Terminated),
            Starting => matches!(next, Running | Failed | Terminated),
            Running => matches!(
                next,
                WaitingForInput
                    | WaitingForPermission
                    | Interrupted
                    | Completed
                    | Failed
                    | Terminated
            ),
            WaitingForInput => matches!(
                next,
                Running | Interrupted | Completed | Failed | Terminated
            ),
            WaitingForPermission => matches!(
                next,
                Running | WaitingForInput | Interrupted | Completed | Failed | Terminated
            ),
            Interrupted => matches!(next, Running | Completed | Failed | Terminated),
            Completed | Failed => matches!(next, Terminated),
            Terminated => false,
        }
    }
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = serde_json::to_value(self).unwrap_or_default();
        f.write_str(value.as_str().unwrap_or("unknown"))
    }
}

/// Persistent session metadata. The provider itself remains the authority for
/// native session state; this structure is the platform view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: SessionId,
    pub machine_id: MachineId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    /// Provider adapter id, e.g. `claude`, `codex`, `mock`.
    pub provider: String,
    /// Concrete agent type, e.g. `claude-code`.
    pub agent_type: String,
    pub status: SessionStatus,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<ApiError>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

impl AgentSession {
    pub fn new(
        machine_id: impl Into<MachineId>,
        provider: impl Into<String>,
        agent_type: impl Into<String>,
        cwd: impl Into<String>,
    ) -> Self {
        let now = crate::common::now();
        Self {
            id: crate::common::new_id("session"),
            machine_id: machine_id.into(),
            workspace_id: None,
            provider: provider.into(),
            agent_type: agent_type.into(),
            status: SessionStatus::Creating,
            cwd: cwd.into(),
            title: None,
            created_at: now,
            updated_at: now,
            last_error: None,
            metadata: serde_json::Value::Null,
        }
    }
}

/// Input used by the runtime to start a provider session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentStartConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    /// Working directory; resolved by the daemon and validated against workspace roots.
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_rules() {
        use SessionStatus::*;
        assert!(Running.can_transition_to(WaitingForPermission));
        assert!(WaitingForPermission.can_transition_to(Running));
        assert!(WaitingForPermission.can_transition_to(WaitingForInput));
        assert!(Interrupted.can_transition_to(Running));
        assert!(Completed.can_transition_to(Terminated));
        assert!(!Completed.can_transition_to(Running));
        assert!(!Terminated.can_transition_to(Running));
        assert!(!Creating.can_transition_to(Running));
    }

    #[test]
    fn status_wire_format() {
        assert_eq!(
            serde_json::to_string(&SessionStatus::WaitingForPermission).unwrap(),
            "\"waiting_for_permission\""
        );
    }
}

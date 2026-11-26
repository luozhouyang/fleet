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
    WaitingForQuestion,
    Interrupted,
    Completed,
    Failed,
    Terminated,
}

impl SessionStatus {
    /// Stable snake_case name used by the wire protocol and filters.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::WaitingForInput => "waiting_for_input",
            Self::WaitingForPermission => "waiting_for_permission",
            Self::WaitingForQuestion => "waiting_for_question",
            Self::Interrupted => "interrupted",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Terminated => "terminated",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Terminated)
    }

    /// Whether a live provider process may still own work for this session.
    ///
    /// `Interrupted` is resumable but no process is active, so it must not
    /// block workspace deletion or count as a running session.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Creating
                | Self::Starting
                | Self::Running
                | Self::WaitingForInput
                | Self::WaitingForPermission
                | Self::WaitingForQuestion
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
                    | WaitingForQuestion
                    | Interrupted
                    | Completed
                    | Failed
                    | Terminated
            ),
            WaitingForInput => matches!(
                next,
                Running | WaitingForQuestion | Interrupted | Completed | Failed | Terminated
            ),
            WaitingForPermission => matches!(
                next,
                Running
                    | WaitingForInput
                    | WaitingForQuestion
                    | Interrupted
                    | Completed
                    | Failed
                    | Terminated
            ),
            WaitingForQuestion => matches!(
                next,
                Running
                    | WaitingForInput
                    | WaitingForPermission
                    | Interrupted
                    | Completed
                    | Failed
                    | Terminated
            ),
            Interrupted => matches!(next, Running | Completed | Failed | Terminated),
            // A failed session may be recovered by a driver that implements a
            // real resume (for example reconnecting an ACP native session).
            Failed => matches!(next, Running | Terminated),
            Completed => matches!(next, Terminated),
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

/// Per-user presentation state attached by the admin.
///
/// The daemon never produces or persists this; it is overlaid on responses for
/// the calling user. A daemon round-trip drops it, which is why the admin must
/// attach it to both list and detail responses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUiState {
    /// Pinned to the top of the caller's session rail.
    #[serde(default)]
    pub pinned: bool,
    /// Position within the pinned group; `None` sorts after ranked pins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_rank: Option<i64>,
    /// Hidden from the default list without closing the session.
    #[serde(default)]
    pub archived: bool,
    /// Activity the caller has not read yet.
    #[serde(default)]
    pub unread: bool,
    /// Free-form tags shown as chips and usable as filters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Folder the caller filed this session under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
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
    /// Caller-specific presentation state. Admin-owned; absent from daemon
    /// responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<SessionUiState>,
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
            ui: None,
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
    /// Registered workspace root, when the daemon resolved the session through one.
    ///
    /// This is daemon-owned context (the session request carries only the
    /// workspace id), used for `cwd_policy = "workspace_root"` and to scope
    /// ACP v1 filesystem requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
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

/// Request to update mutable session metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSessionRequest {
    /// New display title. `None` or an empty string clears the custom title.
    #[serde(default)]
    pub title: Option<String>,
}

/// Request to update the caller's presentation state for one session.
///
/// Every field is optional; `None` leaves the current value untouched. Labels
/// replace the whole set, and an empty list clears them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSessionUiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<Option<String>>,
}

impl UpdateSessionUiRequest {
    /// Whether the request changes anything.
    pub fn is_empty(&self) -> bool {
        self.pinned.is_none()
            && self.archived.is_none()
            && self.labels.is_none()
            && self.folder_id.is_none()
    }
}

/// Request to apply one presentation-state patch to several sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkUpdateSessionUiRequest {
    pub ids: Vec<SessionId>,
    #[serde(flatten)]
    pub patch: UpdateSessionUiRequest,
}

/// Maximum labels per session and their length.
pub const MAX_SESSION_LABELS: usize = 12;
/// Maximum characters in one session label.
pub const MAX_SESSION_LABEL_CHARS: usize = 32;

/// Normalize and validate a label list.
///
/// Labels are trimmed, lowercased, de-duplicated and length-checked so the
/// same word written two ways does not create two filters.
pub fn normalize_session_labels(
    labels: impl IntoIterator<Item = String>,
) -> Result<Vec<String>, ApiError> {
    let mut normalized = Vec::new();
    for label in labels {
        let label = label.trim().to_lowercase();
        if label.is_empty() || normalized.contains(&label) {
            continue;
        }
        if label.chars().count() > MAX_SESSION_LABEL_CHARS {
            return Err(ApiError::invalid(format!(
                "session label `{label}` is longer than {MAX_SESSION_LABEL_CHARS} characters"
            )));
        }
        if normalized.len() >= MAX_SESSION_LABELS {
            return Err(ApiError::invalid(format!(
                "at most {MAX_SESSION_LABELS} labels are allowed per session"
            )));
        }
        normalized.push(label);
    }
    Ok(normalized)
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
    fn session_labels_are_normalized() {
        let labels = normalize_session_labels([
            "Backend".to_string(),
            " backend ".to_string(),
            String::new(),
            "review".to_string(),
        ])
        .expect("labels");
        assert_eq!(labels, vec!["backend".to_string(), "review".to_string()]);

        assert!(normalize_session_labels(vec!["x".repeat(MAX_SESSION_LABEL_CHARS + 1)]).is_err());
        let too_many = (0..=MAX_SESSION_LABELS).map(|index| format!("l{index}"));
        assert!(normalize_session_labels(too_many).is_err());
    }

    #[test]
    fn session_ui_state_is_omitted_when_absent() {
        let session = AgentSession::new("m_1", "mock", "mock", "/tmp");
        let value = serde_json::to_value(&session).expect("serialize");
        assert!(value.get("ui").is_none());

        // A daemon payload without `ui` deserializes unchanged.
        let decoded: AgentSession = serde_json::from_value(value).expect("deserialize");
        assert!(decoded.ui.is_none());

        let mut with_ui = session;
        with_ui.ui = Some(SessionUiState {
            pinned: true,
            ..SessionUiState::default()
        });
        let value = serde_json::to_value(&with_ui).expect("serialize");
        assert_eq!(value["ui"]["pinned"], true);
        assert!(value["ui"].get("pinned_rank").is_none());
    }

    #[test]
    fn status_wire_format() {
        use SessionStatus::*;
        assert_eq!(
            serde_json::to_string(&WaitingForPermission).unwrap(),
            "\"waiting_for_permission\""
        );
        assert_eq!(
            serde_json::to_string(&WaitingForQuestion).unwrap(),
            "\"waiting_for_question\""
        );
        // Filters compare against the same stable names the wire uses.
        for status in [
            Creating,
            Starting,
            Running,
            WaitingForInput,
            WaitingForPermission,
            WaitingForQuestion,
            Interrupted,
            Completed,
            Failed,
            Terminated,
        ] {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{}\"", status.as_str())
            );
        }
    }

    #[test]
    fn question_status_transitions() {
        use SessionStatus::*;
        assert!(Running.can_transition_to(WaitingForQuestion));
        assert!(WaitingForQuestion.can_transition_to(Running));
        assert!(WaitingForQuestion.can_transition_to(WaitingForInput));
        assert!(WaitingForQuestion.is_active());
    }
}

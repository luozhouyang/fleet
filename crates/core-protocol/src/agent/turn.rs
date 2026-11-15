use serde::{Deserialize, Serialize};

use crate::common::{ApiError, SessionId, Timestamp, TurnId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Pending,
    Running,
    Completed,
    Interrupted,
    Failed,
}

impl TurnStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Interrupted | Self::Failed)
    }
}

/// One agent turn: the handling of a single user input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTurn {
    pub id: TurnId,
    pub session_id: SessionId,
    pub status: TurnStatus,
    pub started_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

impl AgentTurn {
    pub fn new(session_id: impl Into<SessionId>) -> Self {
        Self {
            id: crate::common::new_id("turn"),
            session_id: session_id.into(),
            status: TurnStatus::Pending,
            started_at: crate::common::now(),
            completed_at: None,
            error: None,
        }
    }
}

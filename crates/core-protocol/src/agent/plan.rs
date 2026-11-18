use serde::{Deserialize, Serialize};

use crate::common::{SessionId, Timestamp, TurnId};

/// Status of one plan entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

/// Relative priority of one plan entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryPriority {
    High,
    Medium,
    Low,
    #[default]
    Unknown,
}

/// One item of an agent plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    /// Stable entry id when the agent provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Human-readable step.
    pub content: String,
    /// Current status.
    pub status: PlanEntryStatus,
    /// Priority when the agent reports one.
    #[serde(default)]
    pub priority: PlanEntryPriority,
}

/// An agent plan.
///
/// Plans are first-class Fleet domain objects: they are never mixed into
/// messages. Storage keeps the latest plan in session metadata until a
/// dedicated table is introduced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPlan {
    /// Fleet plan id.
    pub id: String,
    /// Session that owns the plan.
    pub session_id: SessionId,
    /// Turn that produced the plan, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// Native plan id, when the agent uses one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_plan_id: Option<String>,
    /// Ordered plan entries.
    #[serde(default)]
    pub entries: Vec<PlanEntry>,
    /// Last update time.
    pub updated_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_roundtrip() {
        let plan = AgentPlan {
            id: "plan_1".into(),
            session_id: "s_1".into(),
            turn_id: None,
            native_plan_id: Some("native".into()),
            entries: vec![PlanEntry {
                id: None,
                content: "build".into(),
                status: PlanEntryStatus::InProgress,
                priority: PlanEntryPriority::High,
            }],
            updated_at: crate::common::now(),
        };
        let back: AgentPlan = serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        assert_eq!(back, plan);
    }
}

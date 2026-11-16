//! User question domain model.

use serde::{Deserialize, Serialize};

use crate::common::{SessionId, Timestamp, TurnId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Pending,
    Answered,
    Rejected,
    TimedOut,
    Cancelled,
    DeliveryFailed,
}

impl QuestionStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Answered => "answered",
            Self::Rejected => "rejected",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::DeliveryFailed => "delivery_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentQuestion {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub question: String,
    #[serde(default)]
    pub options: Vec<AgentQuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default)]
    pub allow_custom: bool,
    #[serde(default)]
    pub is_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentQuestionRequest {
    pub id: String,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub questions: Vec<AgentQuestion>,
    pub status: QuestionStatus,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentQuestionAnswer {
    pub id: String,
    #[serde(default)]
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionDecision {
    pub question_id: String,
    /// Present when the decision came from the admin REST route. The runtime
    /// uses it to reject a mismatched session/question pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub answers: Vec<AgentQuestionAnswer>,
    #[serde(default)]
    pub reject: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RespondQuestionRequest {
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Vec<AgentQuestionAnswer>>,
    #[serde(default)]
    pub reject: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListQuestionsRequest {
    pub session_id: SessionId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::now;

    #[test]
    fn question_request_round_trips_with_snake_case_status() {
        let request = AgentQuestionRequest {
            id: "question_1".into(),
            session_id: "session_1".into(),
            turn_id: Some("turn_1".into()),
            tool_call_id: Some("call_1".into()),
            questions: vec![AgentQuestion {
                id: "target".into(),
                header: Some("Choose".into()),
                question: "Which environment?".into(),
                options: vec![AgentQuestionOption {
                    label: "staging".into(),
                    description: Some("Safe".into()),
                }],
                multi_select: false,
                allow_custom: true,
                is_secret: false,
            }],
            status: QuestionStatus::Pending,
            created_at: now(),
            resolved_at: None,
            expires_at: None,
        };

        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["status"], "pending");
        assert_eq!(value["questions"][0]["multi_select"], false);
        let decoded: AgentQuestionRequest = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn question_status_wire_names_are_stable() {
        assert_eq!(
            serde_json::to_string(&QuestionStatus::TimedOut).unwrap(),
            "\"timed_out\""
        );
        assert!(QuestionStatus::Answered.is_terminal());
        assert!(!QuestionStatus::Pending.is_terminal());
    }
}

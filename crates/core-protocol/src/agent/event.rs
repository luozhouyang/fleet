use serde::{Deserialize, Serialize};

use crate::common::{ApiError, EventId, MessageId, SessionId, Timestamp, TurnId};

use super::{
    AgentMessage, AgentPermissionRequest, AgentQuestionRequest, AgentToolCall, MessageRole,
    SessionStatus, ToolCallStatus,
};

/// Unified event types. Wire names match the protocol reference in
/// `apps/docs/src/content/docs/reference/protocol.mdx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEventType {
    #[serde(rename = "session.created")]
    SessionCreated,
    #[serde(rename = "session.started")]
    SessionStarted,
    #[serde(rename = "session.resumed")]
    SessionResumed,
    #[serde(rename = "session.terminated")]
    SessionTerminated,
    #[serde(rename = "session.deleted")]
    SessionDeleted,
    #[serde(rename = "session.error")]
    SessionError,
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.completed")]
    TurnCompleted,
    #[serde(rename = "turn.interrupted")]
    TurnInterrupted,
    #[serde(rename = "turn.failed")]
    TurnFailed,
    #[serde(rename = "message.started")]
    MessageStarted,
    #[serde(rename = "message.delta")]
    MessageDelta,
    #[serde(rename = "message.completed")]
    MessageCompleted,
    #[serde(rename = "tool.started")]
    ToolStarted,
    #[serde(rename = "tool.input.delta")]
    ToolInputDelta,
    #[serde(rename = "tool.completed")]
    ToolCompleted,
    #[serde(rename = "tool.failed")]
    ToolFailed,
    #[serde(rename = "permission.requested")]
    PermissionRequested,
    #[serde(rename = "permission.approved")]
    PermissionApproved,
    #[serde(rename = "permission.rejected")]
    PermissionRejected,
    #[serde(rename = "question.requested")]
    QuestionRequested,
    #[serde(rename = "question.answered")]
    QuestionAnswered,
    #[serde(rename = "question.rejected")]
    QuestionRejected,
    #[serde(rename = "agent.status.changed")]
    AgentStatusChanged,
    #[serde(rename = "agent.plan.updated")]
    PlanUpdated,
    #[serde(rename = "agent.usage.updated")]
    UsageUpdated,
    #[serde(rename = "agent.activity.changed")]
    ActivityChanged,
    #[serde(rename = "agent.notice")]
    Notice,
    /// Agent-owned terminal output, distinct from a Fleet-created terminal.
    #[serde(rename = "agent.terminal.output")]
    TerminalOutput,
    #[serde(rename = "agent.raw_event")]
    AgentRawEvent,
}

impl AgentEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionCreated => "session.created",
            Self::SessionStarted => "session.started",
            Self::SessionResumed => "session.resumed",
            Self::SessionTerminated => "session.terminated",
            Self::SessionDeleted => "session.deleted",
            Self::SessionError => "session.error",
            Self::TurnStarted => "turn.started",
            Self::TurnCompleted => "turn.completed",
            Self::TurnInterrupted => "turn.interrupted",
            Self::TurnFailed => "turn.failed",
            Self::MessageStarted => "message.started",
            Self::MessageDelta => "message.delta",
            Self::MessageCompleted => "message.completed",
            Self::ToolStarted => "tool.started",
            Self::ToolInputDelta => "tool.input.delta",
            Self::ToolCompleted => "tool.completed",
            Self::ToolFailed => "tool.failed",
            Self::PermissionRequested => "permission.requested",
            Self::PermissionApproved => "permission.approved",
            Self::PermissionRejected => "permission.rejected",
            Self::QuestionRequested => "question.requested",
            Self::QuestionAnswered => "question.answered",
            Self::QuestionRejected => "question.rejected",
            Self::AgentStatusChanged => "agent.status.changed",
            Self::PlanUpdated => "agent.plan.updated",
            Self::UsageUpdated => "agent.usage.updated",
            Self::ActivityChanged => "agent.activity.changed",
            Self::Notice => "agent.notice",
            Self::TerminalOutput => "agent.terminal.output",
            Self::AgentRawEvent => "agent.raw_event",
        }
    }
}

/// A normalized event produced by a provider adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub event_id: EventId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Per-session monotonically increasing sequence number.
    pub sequence: u64,
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub timestamp: Timestamp,
    pub provider: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event: Option<serde_json::Value>,
}

impl AgentEvent {
    pub fn new(
        session_id: impl Into<SessionId>,
        sequence: u64,
        event_type: AgentEventType,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            event_id: crate::common::new_id("evt"),
            session_id: session_id.into(),
            turn_id: None,
            message_id: None,
            tool_call_id: None,
            sequence,
            event_type,
            timestamp: crate::common::now(),
            provider: provider.into(),
            payload: serde_json::Value::Null,
            raw_event: None,
        }
    }

    pub fn with_turn(mut self, turn_id: impl Into<TurnId>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    pub fn with_message(mut self, message_id: impl Into<MessageId>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    pub fn with_tool_call(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }

    pub fn with_payload<T: Serialize>(mut self, payload: &T) -> Self {
        self.payload = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self
    }

    pub fn with_raw_event(mut self, raw: serde_json::Value) -> Self {
        self.raw_event = Some(raw);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaPayload {
    pub message_id: MessageId,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageStartedPayload {
    pub message_id: MessageId,
    pub role: MessageRole,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageCompletedPayload {
    pub message: AgentMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolEventPayload {
    pub tool: AgentToolCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDeltaPayload {
    pub tool_call_id: String,
    pub name: String,
    pub partial_input: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionEventPayload {
    pub permission: AgentPermissionRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionEventPayload {
    pub question: AgentQuestionRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusChangedPayload {
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionErrorPayload {
    pub error: ApiError,
}

/// Payload for `agent.plan.updated`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanUpdatedPayload {
    pub plan: crate::agent::AgentPlan,
}

/// Payload for `agent.usage.updated`.
///
/// The usage shape differs by protocol; the structured numbers Fleet already
/// understands are optional and the raw report is preserved. ACP v2 reports
/// context-window usage as `used` of `size` tokens, so those are the first
/// structured fields. Protocols that only report input/output token counts put
/// them in `raw`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageUpdatedPayload {
    /// Tokens currently used in the reported context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<u64>,
    /// Context size the agent reports against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub raw: serde_json::Value,
}

/// Foreground activity reported by an agent, independent of a Fleet turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityChangedPayload {
    /// `running`, `requires_action`, `idle`, or `unknown`.
    pub state: String,
    /// Turn this activity is attributable to, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// Whether the agent started the activity without a user prompt.
    #[serde(default)]
    pub agent_initiated: bool,
    /// Background work kinds reported by the agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<String>,
}

/// Payload for `agent.notice`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoticePayload {
    /// Notice kind, preserving the agent's discriminator.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// Severity (`info`, `warning`, `error`, `unknown`).
    #[serde(default)]
    pub level: String,
    /// Original payload for forward compatibility.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub raw: serde_json::Value,
}

/// Payload for `agent.terminal.output`.
///
/// These terminals are owned by the agent (ACP v2), not created by Fleet's v1
/// client, so the UI must render them as agent-owned display output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TerminalOutputPayload {
    /// Native terminal id.
    pub terminal_id: String,
    /// Base64-encoded output bytes, exactly as the agent sent them.
    #[serde(default)]
    pub data: String,
    /// Lossy UTF-8 rendering for display.
    #[serde(default)]
    pub text: String,
    /// Command line, when the agent reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Working directory, when the agent reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Exit code, when the terminal has exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Whether this update marks the terminal as exited.
    #[serde(default)]
    pub done: bool,
    /// Original update for forward compatibility.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStatusPayload {
    pub tool_call_id: String,
    pub status: ToolCallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wire_shape_matches_docs() {
        let event = AgentEvent::new("session_1", 1024, AgentEventType::MessageDelta, "claude")
            .with_turn("turn_1")
            .with_message("message_1")
            .with_payload(&serde_json::json!({"text": "Hello"}));
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "message.delta");
        assert_eq!(value["sequence"], 1024);
        assert_eq!(value["payload"]["text"], "Hello");
        assert!(value.get("raw_event").is_none());
    }

    #[test]
    fn all_event_types_serialize() {
        for ty in [
            AgentEventType::SessionCreated,
            AgentEventType::MessageDelta,
            AgentEventType::ToolInputDelta,
            AgentEventType::PermissionRequested,
            AgentEventType::AgentRawEvent,
        ] {
            let json = serde_json::to_string(&ty).unwrap();
            let back: AgentEventType = serde_json::from_str(&json).unwrap();
            assert_eq!(ty, back);
            assert_eq!(json.trim_matches('"'), ty.as_str());
        }
    }
}

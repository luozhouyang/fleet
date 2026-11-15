use serde::{Deserialize, Serialize};

use crate::common::{ApiError, EventId, MessageId, SessionId, Timestamp, TurnId};

use super::{
    AgentMessage, AgentPermissionRequest, AgentToolCall, MessageRole, SessionStatus, ToolCallStatus,
};

/// Unified event types. Wire names match `docs/architecture.md` §6.2.
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
    #[serde(rename = "agent.status.changed")]
    AgentStatusChanged,
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
            Self::AgentStatusChanged => "agent.status.changed",
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
pub struct StatusChangedPayload {
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionErrorPayload {
    pub error: ApiError,
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

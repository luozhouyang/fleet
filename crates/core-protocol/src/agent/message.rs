use serde::{Deserialize, Serialize};

use crate::common::{MessageId, SessionId, Timestamp, TurnId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// A conversation message. Streaming deltas are carried by [`AgentEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: MessageId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub role: MessageRole,
    pub content: String,
    pub created_at: Timestamp,
}

impl AgentMessage {
    pub fn new(
        session_id: impl Into<SessionId>,
        turn_id: Option<TurnId>,
        role: MessageRole,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: crate::common::new_id("message"),
            session_id: session_id.into(),
            turn_id,
            role,
            content: content.into(),
            created_at: crate::common::now(),
        }
    }
}

/// Content attached to a user message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Base64-encoded bytes.
    pub data_base64: String,
}

/// Request to send user input to a running session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

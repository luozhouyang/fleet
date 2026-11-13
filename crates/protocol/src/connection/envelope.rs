use serde::{Deserialize, Serialize};

use crate::common::{ApiError, RequestId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Request,
    Response,
    Event,
    Error,
}

/// The single envelope used for every message on the reverse connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionEnvelope {
    pub version: u16,
    pub stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<RequestId>,
    #[serde(rename = "type")]
    pub message_type: MessageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl ConnectionEnvelope {
    pub fn request(method: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            version: crate::PROTOCOL_VERSION,
            stream_id: "default".into(),
            message_id: Some(crate::common::new_id("msg")),
            message_type: MessageType::Request,
            method: Some(method.into()),
            event: None,
            sequence: None,
            payload,
        }
    }

    pub fn request_on(
        stream_id: impl Into<String>,
        method: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        let mut env = Self::request(method, payload);
        env.stream_id = stream_id.into();
        env
    }

    pub fn response(message_id: impl Into<RequestId>, payload: serde_json::Value) -> Self {
        Self {
            version: crate::PROTOCOL_VERSION,
            stream_id: "default".into(),
            message_id: Some(message_id.into()),
            message_type: MessageType::Response,
            method: None,
            event: None,
            sequence: None,
            payload,
        }
    }

    pub fn error(message_id: Option<RequestId>, error: ApiError) -> Self {
        Self {
            version: crate::PROTOCOL_VERSION,
            stream_id: "default".into(),
            message_id,
            message_type: MessageType::Error,
            method: None,
            event: None,
            sequence: None,
            payload: serde_json::to_value(error).unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn event(event: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            version: crate::PROTOCOL_VERSION,
            stream_id: "events".into(),
            message_id: None,
            message_type: MessageType::Event,
            method: None,
            event: Some(event.into()),
            sequence: None,
            payload,
        }
    }

    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = Some(sequence);
        self
    }

    pub fn request_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    pub fn decode_error(&self) -> Option<ApiError> {
        if self.message_type != MessageType::Error {
            return None;
        }
        serde_json::from_value(self.payload.clone()).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("envelope serialization cannot fail")
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_matches_docs_shape() {
        let env = ConnectionEnvelope::request(
            "workspace.tree.list",
            serde_json::json!({"workspace_id": "ws_123", "path": "src"}),
        );
        let value = serde_json::to_value(&env).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["type"], "request");
        assert_eq!(value["method"], "workspace.tree.list");
        assert!(value["message_id"].as_str().unwrap().starts_with("msg_"));
        assert_eq!(value["stream_id"], "default");
    }

    #[test]
    fn event_has_no_message_id() {
        let env = ConnectionEnvelope::event("agent.event", serde_json::json!({}));
        let value = serde_json::to_value(&env).unwrap();
        assert_eq!(value["type"], "event");
        assert_eq!(value["event"], "agent.event");
        assert!(value.get("message_id").is_none());
        assert!(value.get("method").is_none());
    }

    #[test]
    fn json_roundtrip() {
        let env =
            ConnectionEnvelope::error(Some("msg_1".into()), ApiError::new("not_found", "gone"));
        let back = ConnectionEnvelope::from_json(&env.to_json()).unwrap();
        assert_eq!(env, back);
        assert_eq!(back.decode_error().unwrap().code, "not_found");
    }
}

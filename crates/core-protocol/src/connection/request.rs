use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Optional per-request behaviour carried in the request payload when needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RequestOptions {
    /// Client-side deadline hint in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Idempotency key for commands that may be retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

impl RequestOptions {
    /// Read options embedded in an RPC payload under `options`.
    ///
    /// Unknown or malformed options are treated as absent: an older daemon
    /// must keep serving a newer admin's payloads.
    pub fn from_payload(payload: &Value) -> Self {
        payload
            .get("options")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }

    /// Embed these options in an RPC payload object.
    ///
    /// This is additive: concrete request structs ignore the extra `options`
    /// field, so an older daemon still deserializes the payload.
    pub fn embed(self, payload: &mut Value) {
        if self == Self::default() {
            return;
        }
        if let Value::Object(map) = payload {
            if let Ok(value) = serde_json::to_value(self) {
                map.insert("options".to_string(), value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn options_round_trip_through_a_payload() {
        let options = RequestOptions {
            timeout_ms: None,
            idempotency_key: Some("key-1".into()),
        };
        let mut payload = json!({"session_id": "s_1"});
        options.clone().embed(&mut payload);
        assert_eq!(payload["options"]["idempotency_key"], "key-1");
        assert_eq!(RequestOptions::from_payload(&payload), options);
    }

    #[test]
    fn default_options_are_not_embedded() {
        let mut payload = json!({"session_id": "s_1"});
        RequestOptions::default().embed(&mut payload);
        assert!(payload.get("options").is_none());
    }

    #[test]
    fn malformed_options_are_ignored() {
        assert_eq!(
            RequestOptions::from_payload(&json!({"options": "nope"})),
            RequestOptions::default()
        );
        assert_eq!(
            RequestOptions::from_payload(&json!({"options": {"timeout_ms": "soon"}})),
            RequestOptions::default()
        );
        assert_eq!(
            RequestOptions::from_payload(&json!({"session_id": "s_1"})),
            RequestOptions::default()
        );
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CancelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

use serde::{Deserialize, Serialize};

use crate::common::ApiError;

/// Payload of a successful response. Errors are sent in an `error` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ResponsePayload {
    pub fn ok(data: serde_json::Value) -> Self {
        Self { data: Some(data) }
    }

    pub fn empty() -> Self {
        Self { data: None }
    }
}

/// Error envelope payload (identical to [`ApiError`]).
pub type ErrorPayload = ApiError;

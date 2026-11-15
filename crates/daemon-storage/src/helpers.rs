//! Shared SQLite formatting, parsing and JSON helpers.

use chrono::{DateTime, Utc};
use fleet_core_common::Timestamp;
use fleet_core_protocol::agent::{PermissionStatus, SessionStatus};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

pub(crate) fn bool_to_int(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

pub(crate) fn format_ts(value: Timestamp) -> String {
    value.to_rfc3339()
}

pub(crate) fn format_opt_ts(value: Option<Timestamp>) -> Option<String> {
    value.map(format_ts)
}

pub(crate) fn parse_ts(value: &str) -> Timestamp {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .unwrap_or_default()
}

pub(crate) fn parse_opt_ts(value: Option<String>) -> Option<Timestamp> {
    value.as_deref().map(parse_ts)
}

pub(crate) fn ts_to_nanos(value: Timestamp) -> i64 {
    value
        .timestamp_nanos_opt()
        .unwrap_or_else(|| value.timestamp_millis().saturating_mul(1_000_000))
}

pub(crate) fn nanos_to_ts(value: i64) -> Timestamp {
    DateTime::<Utc>::from_timestamp_nanos(value)
}

pub(crate) fn decode_json_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

pub(crate) fn decode_opt_json<T: DeserializeOwned>(raw: Option<String>) -> Option<T> {
    raw.and_then(|value| serde_json::from_str(&value).ok())
}

pub(crate) fn enum_text<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub(crate) fn enum_from_text<T: DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(Value::String(text.to_owned())).ok()
}

pub(crate) fn parse_session_status(text: &str) -> SessionStatus {
    enum_from_text(text).unwrap_or(SessionStatus::Failed)
}

pub(crate) fn parse_permission_status(text: &str) -> PermissionStatus {
    enum_from_text(text).unwrap_or(PermissionStatus::Cancelled)
}

/// Escape `%`, `_` and `\` so a user path can be embedded in a LIKE pattern.
pub(crate) fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

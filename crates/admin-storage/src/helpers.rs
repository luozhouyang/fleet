//! Shared SQLite formatting, parsing and JSON helpers.

use chrono::{DateTime, Utc};
use fleet_core_common::Timestamp;
use fleet_core_protocol::agent::SessionStatus;
use fleet_core_protocol::machine::MachineStatus;
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

pub(crate) fn decode_json_or_default<T: DeserializeOwned + Default>(raw: &str) -> T {
    serde_json::from_str(raw).unwrap_or_default()
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

pub(crate) fn machine_status_text(status: MachineStatus) -> &'static str {
    match status {
        MachineStatus::Online => "online",
        MachineStatus::Offline => "offline",
        MachineStatus::Degraded => "degraded",
        MachineStatus::Unknown => "unknown",
    }
}

pub(crate) fn parse_machine_status(text: &str) -> MachineStatus {
    match text {
        "online" => MachineStatus::Online,
        "offline" => MachineStatus::Offline,
        "degraded" => MachineStatus::Degraded,
        _ => MachineStatus::Unknown,
    }
}

pub(crate) fn parse_session_status(text: &str) -> SessionStatus {
    enum_from_text(text).unwrap_or(SessionStatus::Failed)
}


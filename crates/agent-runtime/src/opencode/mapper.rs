#![allow(missing_docs)]

//! Mapping from OpenCode v2 SSE events to provider-neutral actions.
//!
//! Pure functions only; dispatch and state live in `adapter.rs`.

use agent_fleet_common::now;
use agent_fleet_protocol::agent::{AgentPermissionRequest, PermissionStatus};
use serde_json::Value;

/// Provider-neutral action derived from one OpenCode event.
#[derive(Debug, Clone, PartialEq)]
pub enum MappedEvent {
    TextStarted {
        message_id: String,
        ordinal: u64,
    },
    TextDelta {
        message_id: String,
        delta: String,
        ordinal: u64,
    },
    TextEnded {
        message_id: String,
        text: String,
        ordinal: u64,
    },
    ToolCalled {
        id: String,
        name: String,
        input: Value,
    },
    ToolSucceeded {
        id: String,
        content: Value,
    },
    ToolFailed {
        id: String,
        error: String,
    },
    ContentUpdated {
        content: Vec<Value>,
    },
    ExecutionStarted,
    ExecutionSucceeded,
    ExecutionInterrupted {
        reason: String,
    },
    ExecutionFailed,
    PermissionAsked {
        request: AgentPermissionRequest,
    },
    PermissionReplied {
        request_id: String,
        reply: String,
    },
    SessionDeleted,
    Idle,
    Other {
        event_type: String,
    },
}

/// One parsed OpenCode event with its native session id, when present.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEvent {
    pub event_type: String,
    pub session_id: Option<String>,
    pub mapped: MappedEvent,
}

/// Parse and map an SSE event payload.
pub fn map_event(value: &Value) -> Option<ParsedEvent> {
    let event_type = value.get("type")?.as_str()?.to_string();
    let data = value.get("data").unwrap_or(&Value::Null);
    let session_id = data
        .get("sessionID")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mapped = match event_type.as_str() {
        "session.text.started" => MappedEvent::TextStarted {
            message_id: string_field(data, "assistantMessageID")?,
            ordinal: u64_field(data, "ordinal"),
        },
        "session.text.delta" => MappedEvent::TextDelta {
            message_id: string_field(data, "assistantMessageID")?,
            delta: string_field(data, "delta")?,
            ordinal: u64_field(data, "ordinal"),
        },
        "session.text.ended" => MappedEvent::TextEnded {
            message_id: string_field(data, "assistantMessageID")?,
            text: string_field(data, "text")?,
            ordinal: u64_field(data, "ordinal"),
        },
        "session.tool.called" => MappedEvent::ToolCalled {
            id: string_field(data, "id")?,
            name: tool_name(data),
            input: data.get("input").cloned().unwrap_or(Value::Null),
        },
        "session.tool.success" => MappedEvent::ToolSucceeded {
            id: string_field(data, "id")?,
            content: data.get("content").cloned().unwrap_or(Value::Null),
        },
        "session.tool.failed" => MappedEvent::ToolFailed {
            id: string_field(data, "id")?,
            error: data
                .get("error")
                .and_then(|error| {
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .or_else(|| error.as_str())
                })
                .unwrap_or("tool failed")
                .to_string(),
        },
        "session.message.content.updated" => MappedEvent::ContentUpdated {
            content: data
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        },
        "session.execution.started" => MappedEvent::ExecutionStarted,
        "session.execution.succeeded" => MappedEvent::ExecutionSucceeded,
        "session.execution.interrupted" => MappedEvent::ExecutionInterrupted {
            reason: data
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("user")
                .to_string(),
        },
        "session.execution.failed" => MappedEvent::ExecutionFailed,
        "session.idle" => MappedEvent::Idle,
        "session.deleted" => MappedEvent::SessionDeleted,
        "permission.asked" => {
            let id = string_field(data, "id")?;
            let session_id = session_id.clone()?;
            let action = data
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let resources: Vec<String> = data
                .get("resources")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let description = data
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if resources.is_empty() {
                        format!("Allow OpenCode to run `{action}`?")
                    } else {
                        format!(
                            "Allow OpenCode to run `{action}` on {}?",
                            resources.join(", ")
                        )
                    }
                });
            MappedEvent::PermissionAsked {
                request: AgentPermissionRequest {
                    id,
                    session_id,
                    turn_id: None,
                    tool_call_id: None,
                    tool_name: action,
                    description,
                    input: data.clone(),
                    status: PermissionStatus::Pending,
                    created_at: now(),
                    resolved_at: None,
                    expires_at: None,
                },
            }
        }
        "permission.replied" => MappedEvent::PermissionReplied {
            request_id: string_field(data, "requestID")?,
            reply: string_field(data, "reply")?,
        },
        other => MappedEvent::Other {
            event_type: other.to_string(),
        },
    };

    Some(ParsedEvent {
        event_type,
        session_id,
        mapped,
    })
}

/// Parse a v2 content array into tool lifecycle facts.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolPart {
    Running {
        id: String,
        name: String,
        input: Value,
    },
    Completed {
        id: String,
        name: String,
        input: Value,
        content: Value,
    },
    Failed {
        id: String,
        name: String,
        input: Value,
        error: String,
    },
}

pub fn tool_parts(content: &[Value]) -> Vec<ToolPart> {
    let mut parts = Vec::new();
    for item in content {
        if item.get("type").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let state = item.get("state").unwrap_or(&Value::Null);
        let input = state.get("input").cloned().unwrap_or(Value::Null);
        match state.get("status").and_then(Value::as_str) {
            Some("completed") => parts.push(ToolPart::Completed {
                id: id.to_string(),
                name,
                input,
                content: state.get("content").cloned().unwrap_or(Value::Null),
            }),
            Some("error") | Some("failed") => parts.push(ToolPart::Failed {
                id: id.to_string(),
                name: name.clone(),
                input,
                error: state
                    .get("error")
                    .and_then(|error| {
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .or_else(|| error.as_str())
                    })
                    .unwrap_or("tool failed")
                    .to_string(),
            }),
            _ => parts.push(ToolPart::Running {
                id: id.to_string(),
                name,
                input,
            }),
        }
    }
    parts
}

fn string_field(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(Value::as_str).map(str::to_string)
}

fn u64_field(data: &Value, key: &str) -> u64 {
    data.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn tool_name(data: &Value) -> String {
    data.get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            data.get("native")
                .and_then(|native| native.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or("tool")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<Value> {
        let raw = match name {
            "text_stream.jsonl" => include_str!("fixtures/text_stream.jsonl"),
            "tool_permission.jsonl" => include_str!("fixtures/tool_permission.jsonl"),
            "execution.jsonl" => include_str!("fixtures/execution.jsonl"),
            other => panic!("unknown fixture {other}"),
        };
        raw.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("fixture line is valid JSON"))
            .collect()
    }

    #[test]
    fn maps_text_stream_fixture() {
        let events: Vec<ParsedEvent> = fixture("text_stream.jsonl")
            .iter()
            .filter_map(map_event)
            .collect();
        assert_eq!(events.len(), 6);
        assert_eq!(events[0].session_id.as_deref(), Some("ses_demo"));
        assert_eq!(
            events[0].mapped,
            MappedEvent::TextStarted {
                message_id: "msg_a1".into(),
                ordinal: 0
            }
        );
        assert_eq!(
            events[1].mapped,
            MappedEvent::TextDelta {
                message_id: "msg_a1".into(),
                delta: "Hello ".into(),
                ordinal: 0
            }
        );
        assert!(matches!(
            events[3].mapped,
            MappedEvent::TextEnded { ref text, .. } if text == "Hello world"
        ));
        assert_eq!(events[4].mapped, MappedEvent::ExecutionSucceeded);
        assert_eq!(events[5].mapped, MappedEvent::Idle);
    }

    #[test]
    fn maps_tool_and_permission_fixture() {
        let events: Vec<ParsedEvent> = fixture("tool_permission.jsonl")
            .iter()
            .filter_map(map_event)
            .collect();
        assert!(matches!(
            events[0].mapped,
            MappedEvent::ToolCalled { ref id, ref name, .. } if id == "tool_1" && name == "tool"
        ));
        match &events[1].mapped {
            MappedEvent::PermissionAsked { request } => {
                assert_eq!(request.id, "per_1");
                assert_eq!(request.session_id, "ses_demo");
                assert_eq!(request.tool_name, "bash");
                assert!(request.description.contains("echo hi"));
                assert_eq!(request.status, PermissionStatus::Pending);
            }
            other => panic!("expected permission.asked, got {other:?}"),
        }
        assert_eq!(
            events[2].mapped,
            MappedEvent::PermissionReplied {
                request_id: "per_1".into(),
                reply: "once".into()
            }
        );
        assert!(matches!(
            events[3].mapped,
            MappedEvent::ToolSucceeded { ref id, .. } if id == "tool_1"
        ));
        match &events[4].mapped {
            MappedEvent::ContentUpdated { content } => {
                let parts = tool_parts(content);
                assert_eq!(parts.len(), 1);
                assert!(matches!(parts[0], ToolPart::Completed { ref id, .. } if id == "tool_1"));
            }
            other => panic!("expected content update, got {other:?}"),
        }
    }

    #[test]
    fn maps_execution_interrupt_fixture() {
        let events: Vec<ParsedEvent> = fixture("execution.jsonl")
            .iter()
            .filter_map(map_event)
            .collect();
        assert_eq!(events[0].mapped, MappedEvent::ExecutionStarted);
        assert_eq!(
            events[1].mapped,
            MappedEvent::ExecutionInterrupted {
                reason: "user".into()
            }
        );
    }

    #[test]
    fn unknown_events_are_preserved() {
        let parsed = map_event(&serde_json::json!({
            "type": "session.reasoning.delta",
            "data": {"sessionID": "ses_1", "delta": "thinking"}
        }))
        .unwrap();
        assert_eq!(
            parsed.mapped,
            MappedEvent::Other {
                event_type: "session.reasoning.delta".into()
            }
        );
    }
}

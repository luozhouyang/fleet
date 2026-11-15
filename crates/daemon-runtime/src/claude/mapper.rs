//! Mapping from Claude stream-json lines to unified agent events.

use fleet_core_common::{new_id, now, ApiError};
use fleet_core_protocol::agent::{
    AgentMessage, AgentToolCall, MessageRole, SessionStatus, ToolCallStatus,
};
use serde_json::Value;

use super::protocol::{
    tool_result_value, AssistantMessage, ClaudeLine, ContentBlock, MessageContent, StreamEvent,
};

/// Context needed to build unified DTOs from native lines.
#[derive(Debug, Clone)]
pub(crate) struct MapContext {
    pub session_id: String,
    pub turn_id: Option<String>,
}

impl MapContext {
    pub(crate) fn new(session_id: impl Into<String>, turn_id: Option<String>) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id,
        }
    }
}

/// A normalized action produced from one Claude line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MappedEvent {
    /// `system/init` line carrying the native session id.
    SessionInit { session_id: String },
    /// Assistant message began.
    MessageStarted {
        message_id: String,
        role: MessageRole,
    },
    /// Assistant text delta.
    MessageDelta { message_id: String, text: String },
    /// Assistant message completed.
    MessageCompleted { message: AgentMessage },
    /// Tool invocation started.
    ToolStarted { tool: AgentToolCall },
    /// Tool invocation completed successfully.
    ToolCompleted { tool: AgentToolCall },
    /// Tool invocation failed.
    ToolFailed { tool: AgentToolCall, error: String },
    /// Turn completed successfully.
    TurnCompleted,
    /// Turn failed.
    TurnFailed { error: ApiError },
    /// Session status update implied by the line.
    Status(SessionStatus),
    /// A text delta from `stream_event`.
    StreamTextDelta(String),
    /// The native id of the message being streamed, from `message_start`.
    StreamMessageStart { message_id: String },
    /// The streaming message ended.
    StreamMessageStop,
    /// Unrecognized native data, forwarded verbatim.
    Raw(Value),
}

/// Map one parsed line into zero or more unified actions.
pub(crate) fn map_line(line: &ClaudeLine, ctx: &MapContext) -> Vec<MappedEvent> {
    match line {
        ClaudeLine::System {
            subtype,
            session_id,
            ..
        } => {
            if subtype.as_deref() == Some("init") {
                if let Some(session_id) = session_id {
                    return vec![MappedEvent::SessionInit {
                        session_id: session_id.clone(),
                    }];
                }
            }
            Vec::new()
        }
        ClaudeLine::Assistant { message, .. } => map_assistant(message, ctx),
        ClaudeLine::User { message, .. } => map_user_content(&message.content, ctx),
        ClaudeLine::ResultLine {
            is_error, result, ..
        } => {
            if is_error.unwrap_or(false) {
                vec![
                    MappedEvent::TurnFailed {
                        error: ApiError::new(
                            "claude_result_error",
                            result
                                .clone()
                                .unwrap_or_else(|| "Claude reported an error".to_string()),
                        ),
                    },
                    MappedEvent::Status(SessionStatus::Failed),
                ]
            } else {
                vec![
                    MappedEvent::TurnCompleted,
                    MappedEvent::Status(SessionStatus::WaitingForInput),
                ]
            }
        }
        ClaudeLine::StreamEvent { event, .. } => map_stream_event(event),
        ClaudeLine::Unknown => Vec::new(),
    }
}

fn map_assistant(message: &AssistantMessage, ctx: &MapContext) -> Vec<MappedEvent> {
    let mut events = Vec::new();
    let mut text = String::new();
    let mut tools = Vec::new();

    for block in message.content.blocks() {
        match block {
            ContentBlock::Text { text: value } => text.push_str(&value),
            ContentBlock::ToolUse { id, name, input } => {
                tools.push(AgentToolCall {
                    id,
                    session_id: ctx.session_id.clone(),
                    turn_id: ctx.turn_id.clone(),
                    name,
                    status: ToolCallStatus::Running,
                    input,
                    output: Value::Null,
                    error: None,
                    started_at: now(),
                    completed_at: None,
                });
            }
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } => {}
            ContentBlock::Unknown => {}
        }
    }

    if !text.is_empty() {
        let message_id = message.id.clone().unwrap_or_else(|| new_id("message"));
        let unified = AgentMessage {
            id: message_id.clone(),
            session_id: ctx.session_id.clone(),
            turn_id: ctx.turn_id.clone(),
            role: MessageRole::Assistant,
            content: text.clone(),
            created_at: now(),
        };
        events.push(MappedEvent::MessageStarted {
            message_id: message_id.clone(),
            role: MessageRole::Assistant,
        });
        events.push(MappedEvent::MessageDelta {
            message_id: message_id.clone(),
            text,
        });
        events.push(MappedEvent::MessageCompleted { message: unified });
    }

    for tool in tools {
        events.push(MappedEvent::ToolStarted { tool });
    }

    events
}

fn map_user_content(content: &MessageContent, ctx: &MapContext) -> Vec<MappedEvent> {
    let mut events = Vec::new();
    for block in content.blocks() {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = block
        {
            let output = tool_result_value(content.as_ref());
            let failed = is_error.unwrap_or(false);
            let error = if failed {
                Some(match &output {
                    Value::String(text) if !text.is_empty() => text.clone(),
                    Value::Null => "tool call failed".to_string(),
                    other => other.to_string(),
                })
            } else {
                None
            };
            let tool = AgentToolCall {
                id: tool_use_id,
                session_id: ctx.session_id.clone(),
                turn_id: ctx.turn_id.clone(),
                name: "tool".to_string(),
                status: if failed {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                },
                input: Value::Null,
                output,
                error: error.clone(),
                started_at: now(),
                completed_at: Some(now()),
            };
            if failed {
                events.push(MappedEvent::ToolFailed {
                    tool,
                    error: error.unwrap_or_else(|| "tool call failed".to_string()),
                });
            } else {
                events.push(MappedEvent::ToolCompleted { tool });
            }
        }
    }
    events
}

fn map_stream_event(event: &StreamEvent) -> Vec<MappedEvent> {
    match event {
        StreamEvent::ContentBlockDelta {
            delta: super::protocol::Delta::Text { text },
            ..
        } => vec![MappedEvent::StreamTextDelta(text.clone())],
        StreamEvent::MessageStop => vec![MappedEvent::StreamMessageStop],
        StreamEvent::MessageStart { message } => message
            .as_ref()
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(|id| {
                vec![MappedEvent::StreamMessageStart {
                    message_id: id.to_string(),
                }]
            })
            .unwrap_or_default(),
        StreamEvent::ContentBlockDelta { .. } => vec![MappedEvent::Raw(
            serde_json::to_value(event).unwrap_or(Value::Null),
        )],
        StreamEvent::ContentBlockStart { .. }
        | StreamEvent::ContentBlockStop { .. }
        | StreamEvent::MessageDelta { .. } => Vec::new(),
        StreamEvent::Unknown => vec![MappedEvent::Raw(
            serde_json::to_value(event).unwrap_or(Value::Null),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::protocol::ClaudeLine;

    fn parse(line: &str) -> ClaudeLine {
        serde_json::from_str(line).expect("fixture line should parse")
    }

    const INIT: &str = include_str!("fixtures/init_system.jsonl");
    const ASSISTANT: &str = include_str!("fixtures/assistant_text.jsonl");
    const TOOL: &str = include_str!("fixtures/tool_use.jsonl");
    const ERROR: &str = include_str!("fixtures/error_result.jsonl");
    const STREAM: &str = include_str!("fixtures/partial_delta.jsonl");

    fn lines(fixture: &str) -> Vec<ClaudeLine> {
        fixture
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(parse)
            .collect()
    }

    #[test]
    fn maps_init_session_id() {
        let ctx = MapContext::new("session_1", Some("turn_1".to_string()));
        let events = map_line(&lines(INIT)[0], &ctx);
        assert_eq!(
            events,
            vec![MappedEvent::SessionInit {
                session_id: "native-session-123".to_string()
            }]
        );
    }

    #[test]
    fn maps_assistant_text_and_result() {
        let ctx = MapContext::new("session_1", Some("turn_1".to_string()));
        let parsed = lines(ASSISTANT);
        let first = map_line(&parsed[0], &ctx);
        assert!(matches!(first[0], MappedEvent::MessageStarted { .. }));
        assert!(matches!(first[1], MappedEvent::MessageDelta { .. }));
        match &first[2] {
            MappedEvent::MessageCompleted { message } => {
                assert_eq!(message.content, "I analyzed the project.");
                assert_eq!(message.turn_id.as_deref(), Some("turn_1"));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let last = map_line(parsed.last().expect("result line"), &ctx);
        assert_eq!(last[0], MappedEvent::TurnCompleted);
        assert_eq!(last[1], MappedEvent::Status(SessionStatus::WaitingForInput));
    }

    #[test]
    fn maps_tool_use_and_result() {
        let ctx = MapContext::new("session_1", None);
        let parsed = lines(TOOL);
        let assistant = map_line(&parsed[0], &ctx);
        assert!(assistant.iter().any(
            |event| matches!(event, MappedEvent::ToolStarted { tool } if tool.name == "Bash")
        ));
        let result = map_line(&parsed[1], &ctx);
        match &result[0] {
            MappedEvent::ToolCompleted { tool } => {
                assert_eq!(tool.id, "toolu_01");
                assert_eq!(tool.output, Value::String("hello\n".to_string()));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_error_result() {
        let ctx = MapContext::new("session_1", None);
        let parsed = lines(ERROR);
        let events = map_line(parsed.last().expect("result line"), &ctx);
        match &events[0] {
            MappedEvent::TurnFailed { error } => assert_eq!(error.code, "claude_result_error"),
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(events[1], MappedEvent::Status(SessionStatus::Failed));
    }

    #[test]
    fn maps_stream_message_start_id() {
        let line = parse(
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_123"}}}"#,
        );
        let ctx = MapContext::new("session_1", Some("turn_1".into()));
        let events = map_line(&line, &ctx);
        assert!(matches!(
            &events[0],
            MappedEvent::StreamMessageStart { message_id } if message_id == "msg_123"
        ));
    }

    #[test]
    fn maps_partial_text_deltas() {
        let ctx = MapContext::new("session_1", None);
        let parsed = lines(STREAM);
        let mut text = String::new();
        let mut stopped = false;
        for line in &parsed {
            for event in map_line(line, &ctx) {
                match event {
                    MappedEvent::StreamTextDelta(delta) => text.push_str(&delta),
                    MappedEvent::StreamMessageStop => stopped = true,
                    _ => {}
                }
            }
        }
        assert_eq!(text, "Streaming hello");
        assert!(stopped);
    }
}

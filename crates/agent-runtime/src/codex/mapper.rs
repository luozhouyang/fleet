//! Mapping from Codex app-server notifications to unified agent events.

use agent_fleet_common::{now, ApiError};
use agent_fleet_protocol::agent::{
    AgentMessage, AgentToolCall, MessageRole, SessionStatus, ToolCallStatus,
};
use serde_json::{json, Value};

use super::protocol::{
    AgentMessageDelta, ItemNotification, ThreadItem, ThreadStarted, ThreadStatusChanged,
    TurnNotification,
};

/// Context needed to build unified DTOs from native notifications.
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

/// A normalized action produced from a Codex notification.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MappedEvent {
    /// A thread was started or resumed.
    ThreadStarted { thread_id: String },
    /// Turn started.
    TurnStarted,
    /// Turn completed.
    TurnCompleted,
    /// Turn interrupted.
    TurnInterrupted,
    /// Turn failed.
    TurnFailed { error: ApiError },
    /// Assistant message began.
    MessageStarted { message_id: String },
    /// Assistant text delta.
    MessageDelta { message_id: String, text: String },
    /// Assistant message completed.
    MessageCompleted { message: AgentMessage },
    /// Tool invocation started.
    ToolStarted { tool: AgentToolCall },
    /// Tool invocation completed.
    ToolCompleted { tool: AgentToolCall },
    /// Tool invocation failed.
    ToolFailed { tool: AgentToolCall, error: String },
    /// Session status update.
    Status(SessionStatus),
    /// Unrecognized native data.
    Raw(Value),
}

/// Map one server notification.
pub(crate) fn map_notification(method: &str, params: &Value, ctx: &MapContext) -> Vec<MappedEvent> {
    match method {
        "thread/started" => serde_json::from_value::<ThreadStarted>(params.clone())
            .map(|started| {
                vec![MappedEvent::ThreadStarted {
                    thread_id: started.thread.id,
                }]
            })
            .unwrap_or_else(|_| vec![MappedEvent::Raw(params.clone())]),
        "turn/started" => match serde_json::from_value::<TurnNotification>(params.clone()) {
            Ok(notification) => map_turn_started(&notification),
            Err(_) => vec![MappedEvent::Raw(params.clone())],
        },
        "turn/completed" => match serde_json::from_value::<TurnNotification>(params.clone()) {
            Ok(notification) => map_turn_completed(&notification),
            Err(_) => vec![MappedEvent::Raw(params.clone())],
        },
        "item/started" | "item/completed" => {
            match serde_json::from_value::<ItemNotification>(params.clone()) {
                Ok(notification) => {
                    if method == "item/started" {
                        map_item_started(&notification, ctx)
                    } else {
                        map_item_completed(&notification, ctx)
                    }
                }
                Err(_) => vec![MappedEvent::Raw(params.clone())],
            }
        }
        "item/agentMessage/delta" => {
            match serde_json::from_value::<AgentMessageDelta>(params.clone()) {
                Ok(delta) => vec![MappedEvent::MessageDelta {
                    message_id: delta.item_id,
                    text: delta.delta,
                }],
                Err(_) => vec![MappedEvent::Raw(params.clone())],
            }
        }
        "thread/status/changed" => {
            match serde_json::from_value::<ThreadStatusChanged>(params.clone()) {
                Ok(changed) => map_thread_status(&changed),
                Err(_) => vec![MappedEvent::Raw(params.clone())],
            }
        }
        _ => vec![MappedEvent::Raw(params.clone())],
    }
}

fn map_turn_started(notification: &TurnNotification) -> Vec<MappedEvent> {
    let _ = notification;
    vec![MappedEvent::TurnStarted]
}

fn map_turn_completed(notification: &TurnNotification) -> Vec<MappedEvent> {
    match notification.turn.status.as_str() {
        "completed" => vec![
            MappedEvent::TurnCompleted,
            MappedEvent::Status(SessionStatus::WaitingForInput),
        ],
        "interrupted" => vec![
            MappedEvent::TurnInterrupted,
            MappedEvent::Status(SessionStatus::Interrupted),
        ],
        "failed" => {
            let message = notification
                .turn
                .error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "codex turn failed".to_string());
            vec![
                MappedEvent::TurnFailed {
                    error: ApiError::new("codex_turn_failed", message),
                },
                MappedEvent::Status(SessionStatus::Failed),
            ]
        }
        _ => vec![MappedEvent::TurnStarted],
    }
}

fn map_thread_status(changed: &ThreadStatusChanged) -> Vec<MappedEvent> {
    let status = match changed.status.kind.as_str() {
        "active" => SessionStatus::Running,
        "idle" => SessionStatus::WaitingForInput,
        "systemError" => SessionStatus::Failed,
        _ => return Vec::new(),
    };
    vec![MappedEvent::Status(status)]
}

fn map_item_started(notification: &ItemNotification, ctx: &MapContext) -> Vec<MappedEvent> {
    match &notification.item {
        ThreadItem::AgentMessage { id, .. } => vec![MappedEvent::MessageStarted {
            message_id: id.clone(),
        }],
        ThreadItem::CommandExecution {
            id, command, cwd, ..
        } => vec![MappedEvent::ToolStarted {
            tool: tool_call(
                id,
                "command_execution",
                json!({"command": command, "cwd": cwd}),
                ctx,
            ),
        }],
        ThreadItem::FileChange { id, changes, .. } => vec![MappedEvent::ToolStarted {
            tool: tool_call(id, "file_change", json!({"changes": changes}), ctx),
        }],
        ThreadItem::McpToolCall {
            id,
            server,
            tool,
            arguments,
            ..
        } => vec![MappedEvent::ToolStarted {
            tool: tool_call(id, &format!("{server}.{tool}"), arguments.clone(), ctx),
        }],
        ThreadItem::UserMessage { .. } | ThreadItem::Other => Vec::new(),
    }
}

fn map_item_completed(notification: &ItemNotification, ctx: &MapContext) -> Vec<MappedEvent> {
    match &notification.item {
        ThreadItem::AgentMessage { id, text } => vec![MappedEvent::MessageCompleted {
            message: AgentMessage {
                id: id.clone(),
                session_id: ctx.session_id.clone(),
                turn_id: ctx.turn_id.clone(),
                role: MessageRole::Assistant,
                content: text.clone(),
                created_at: now(),
            },
        }],
        ThreadItem::CommandExecution {
            id,
            status,
            aggregated_output,
            exit_code,
            ..
        } => {
            let output = json!({
                "aggregatedOutput": aggregated_output,
                "exitCode": exit_code,
            });
            let failed = matches!(status.as_str(), "failed" | "declined");
            let mut tool = tool_call(id, "command_execution", Value::Null, ctx);
            tool.output = output;
            tool.completed_at = Some(now());
            if failed {
                tool.status = ToolCallStatus::Failed;
                tool.error = Some(format!("command did not complete successfully ({status})"));
                vec![MappedEvent::ToolFailed {
                    error: tool.error.clone().unwrap_or_default(),
                    tool,
                }]
            } else {
                tool.status = ToolCallStatus::Completed;
                vec![MappedEvent::ToolCompleted { tool }]
            }
        }
        ThreadItem::FileChange {
            id,
            status,
            changes,
        } => {
            let mut tool = tool_call(id, "file_change", Value::Null, ctx);
            tool.output = json!({"changes": changes});
            tool.completed_at = Some(now());
            if matches!(status.as_str(), "failed" | "declined") {
                tool.status = ToolCallStatus::Failed;
                tool.error = Some(format!("file change did not apply ({status})"));
                vec![MappedEvent::ToolFailed {
                    error: tool.error.clone().unwrap_or_default(),
                    tool,
                }]
            } else {
                tool.status = ToolCallStatus::Completed;
                vec![MappedEvent::ToolCompleted { tool }]
            }
        }
        ThreadItem::McpToolCall {
            id,
            server,
            tool: name,
            status,
            error,
            result,
            ..
        } => {
            let mut call = tool_call(id, &format!("{server}.{name}"), Value::Null, ctx);
            call.output = result.clone().unwrap_or(Value::Null);
            call.completed_at = Some(now());
            if let Some(error) = error {
                call.status = ToolCallStatus::Failed;
                call.error = Some(error.message.clone());
                vec![MappedEvent::ToolFailed {
                    error: error.message.clone(),
                    tool: call,
                }]
            } else if status == "failed" {
                call.status = ToolCallStatus::Failed;
                call.error = Some("MCP tool call failed".to_string());
                vec![MappedEvent::ToolFailed {
                    error: "MCP tool call failed".to_string(),
                    tool: call,
                }]
            } else {
                call.status = ToolCallStatus::Completed;
                vec![MappedEvent::ToolCompleted { tool: call }]
            }
        }
        ThreadItem::UserMessage { .. } | ThreadItem::Other => Vec::new(),
    }
}

fn tool_call(id: &str, name: &str, input: Value, ctx: &MapContext) -> AgentToolCall {
    AgentToolCall {
        id: id.to_string(),
        session_id: ctx.session_id.clone(),
        turn_id: ctx.turn_id.clone(),
        name: name.to_string(),
        status: ToolCallStatus::Running,
        input,
        output: Value::Null,
        error: None,
        started_at: now(),
        completed_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = include_str!("fixtures/thread_start.jsonl");
    const TEXT: &str = include_str!("fixtures/turn_text.jsonl");
    const TOOL: &str = include_str!("fixtures/tool_command.jsonl");
    const ERROR: &str = include_str!("fixtures/error_turn.jsonl");

    fn notifications(fixture: &str) -> Vec<(String, Value)> {
        fixture
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|value| {
                let method = value.get("method")?.as_str()?.to_string();
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                Some((method, params))
            })
            .collect()
    }

    #[test]
    fn maps_thread_started() {
        let ctx = MapContext::new("session_1", None);
        let (method, params) = notifications(THREAD)
            .into_iter()
            .find(|(method, _)| method == "thread/started")
            .expect("thread/started fixture");
        assert_eq!(
            map_notification(&method, &params, &ctx),
            vec![MappedEvent::ThreadStarted {
                thread_id: "thr_123".to_string()
            }]
        );
    }

    #[test]
    fn maps_agent_message_stream() {
        let ctx = MapContext::new("session_1", Some("turn_1".to_string()));
        let mut text = String::new();
        let mut completed = None;
        let mut turn_completed = false;
        for (method, params) in notifications(TEXT) {
            for event in map_notification(&method, &params, &ctx) {
                match event {
                    MappedEvent::MessageDelta { text: delta, .. } => text.push_str(&delta),
                    MappedEvent::MessageCompleted { message } => completed = Some(message),
                    MappedEvent::TurnCompleted => turn_completed = true,
                    _ => {}
                }
            }
        }
        assert_eq!(text, "Hello world");
        assert_eq!(completed.expect("completed message").content, "Hello world");
        assert!(turn_completed);
    }

    #[test]
    fn maps_command_tool_lifecycle() {
        let ctx = MapContext::new("session_1", None);
        let events: Vec<MappedEvent> = notifications(TOOL)
            .into_iter()
            .flat_map(|(method, params)| map_notification(&method, &params, &ctx))
            .collect();
        assert!(events.iter().any(|event| matches!(
            event,
            MappedEvent::ToolStarted { tool } if tool.name == "command_execution"
        )));
        match events
            .iter()
            .find(|event| matches!(event, MappedEvent::ToolCompleted { .. }))
            .expect("tool completion")
        {
            MappedEvent::ToolCompleted { tool } => {
                assert_eq!(tool.output["exitCode"], 0);
                assert_eq!(tool.output["aggregatedOutput"], "hello\n");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn maps_failed_turn() {
        let ctx = MapContext::new("session_1", None);
        let events: Vec<MappedEvent> = notifications(ERROR)
            .into_iter()
            .flat_map(|(method, params)| map_notification(&method, &params, &ctx))
            .collect();
        let failure = events
            .iter()
            .find_map(|event| match event {
                MappedEvent::TurnFailed { error } => Some(error.clone()),
                _ => None,
            })
            .expect("turn failure");
        assert_eq!(failure.code, "codex_turn_failed");
        assert!(events.contains(&MappedEvent::Status(SessionStatus::Failed)));
    }
}

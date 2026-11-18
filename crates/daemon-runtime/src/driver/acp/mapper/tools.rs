//! ACP v2 tool call upsert and content chunk mapping.
//!
//! v2 tool calls are upserts: omitted fields keep their previous value, `null`
//! clears it, and a value replaces it. Fleet creates the tool call on first
//! sight and updates the same Fleet tool call afterwards. A terminal status is
//! never overwritten by a later, older update.

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::v2::schema as v2s;
use fleet_core_protocol::agent::{AgentEventType, AgentToolCall, ToolCallStatus, ToolEventPayload};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::event::AgentEventEmitter;

use super::common::{tool_contents_text, tool_status};
use super::state::{AcpUpdateState, ToolCallMapping};

/// Apply a tool call upsert.
pub fn apply_tool_call_update(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    update: &v2s::ToolCallUpdate,
) {
    let native_id = update.tool_call_id.to_string();
    let mut guard = state.lock();
    let sequence = guard.next_sequence();
    let mapping = guard
        .tools
        .entry(native_id.clone())
        .or_insert_with(|| ToolCallMapping::new(native_id.clone()));
    mapping.last_update_sequence = sequence;

    if let MaybeUndefined::Value(name) = &update.name {
        mapping.name = name.clone();
    } else if let MaybeUndefined::Null = &update.name {
        mapping.name = "tool".to_string();
    }
    if let MaybeUndefined::Value(title) = &update.title {
        if mapping.name == "tool" {
            mapping.name = title.clone();
        }
    }
    if let MaybeUndefined::Value(status) = &update.status {
        let next = tool_status(status.clone());
        // A terminal status is final; late non-terminal updates must not
        // resurrect a finished tool call.
        if !(mapping.is_terminal() && !is_terminal(next)) {
            mapping.status = next;
        }
    } else if let MaybeUndefined::Null = &update.status {
        if !mapping.is_terminal() {
            mapping.status = ToolCallStatus::Pending;
        }
    }
    if let MaybeUndefined::Value(input) = &update.raw_input {
        mapping.input = input.clone();
    } else if let MaybeUndefined::Null = &update.raw_input {
        mapping.input = Value::Null;
    }
    if let MaybeUndefined::Value(output) = &update.raw_output {
        mapping.output = output.clone();
    } else if let MaybeUndefined::Null = &update.raw_output {
        mapping.output = Value::Null;
    }
    if let MaybeUndefined::Value(contents) = &update.content {
        let text = tool_contents_text(contents);
        mapping.content = text;
        if mapping.output.is_null() && !mapping.content.is_empty() {
            mapping.output = Value::String(mapping.content.clone());
        }
    } else if let MaybeUndefined::Null = &update.content {
        mapping.content.clear();
    }
    if mapping.status == ToolCallStatus::Failed && mapping.error.is_none() {
        mapping.error = Some(if mapping.content.is_empty() {
            "the tool call failed".to_string()
        } else {
            mapping.content.clone()
        });
    }
    let tool = to_tool_call(emitter, mapping);
    guard.trim();
    drop(guard);
    emit_tool(emitter, &tool);
}

/// Apply a streamed chunk of tool output.
pub fn apply_tool_content_chunk(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    chunk: &v2s::ToolCallContentChunk,
) {
    let Some(text) = super::common::tool_content_text(&chunk.content) else {
        super::common::emit_raw(emitter, "tool_content", chunk);
        return;
    };
    let native_id = chunk.tool_call_id.to_string();
    let mut guard = state.lock();
    let sequence = guard.next_sequence();
    let mapping = guard
        .tools
        .entry(native_id.clone())
        .or_insert_with(|| ToolCallMapping::new(native_id.clone()));
    mapping.last_update_sequence = sequence;
    mapping.content.push_str(&text);
    mapping.output = Value::String(mapping.content.clone());
    if mapping.status == ToolCallStatus::Pending {
        mapping.status = ToolCallStatus::Running;
    }
    let tool = to_tool_call(emitter, mapping);
    guard.trim();
    drop(guard);
    emit_tool(emitter, &tool);
}

fn to_tool_call(emitter: &AgentEventEmitter, mapping: &ToolCallMapping) -> AgentToolCall {
    let terminal = mapping.is_terminal();
    AgentToolCall {
        id: mapping.fleet_tool_call_id.clone(),
        session_id: emitter.session_id().to_string(),
        turn_id: emitter.current_turn_id(),
        name: mapping.name.clone(),
        status: mapping.status,
        input: mapping.input.clone(),
        output: mapping.output.clone(),
        error: mapping.error.clone(),
        started_at: fleet_core_common::now(),
        completed_at: terminal.then(fleet_core_common::now),
    }
}

fn emit_tool(emitter: &AgentEventEmitter, tool: &AgentToolCall) {
    let (event_type, payload) = match tool.status {
        ToolCallStatus::Completed => (
            AgentEventType::ToolCompleted,
            serde_json::to_value(ToolEventPayload { tool: tool.clone() }).unwrap_or_default(),
        ),
        ToolCallStatus::Failed => (
            AgentEventType::ToolFailed,
            json!({
                "tool": tool.clone(),
                "error": tool
                    .error
                    .clone()
                    .unwrap_or_else(|| "the tool call failed".to_string()),
            }),
        ),
        ToolCallStatus::Cancelled => (
            AgentEventType::ToolFailed,
            json!({
                "tool": tool.clone(),
                "error": "the tool call was cancelled",
            }),
        ),
        ToolCallStatus::Pending | ToolCallStatus::Running => (
            AgentEventType::ToolStarted,
            serde_json::to_value(ToolEventPayload { tool: tool.clone() }).unwrap_or_default(),
        ),
    };
    emitter.emit_with(
        event_type,
        payload,
        tool.turn_id.clone(),
        None,
        Some(tool.id.clone()),
    );
}

fn is_terminal(status: ToolCallStatus) -> bool {
    matches!(
        status,
        ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::acp::tests::test_emitter;

    fn drain(
        receiver: &mut tokio::sync::broadcast::Receiver<fleet_core_protocol::agent::AgentEvent>,
    ) -> Vec<fleet_core_protocol::agent::AgentEvent> {
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    #[test]
    fn patch_fields_preserve_earlier_values() {
        let emitter = test_emitter("s1");
        let state = Mutex::new(AcpUpdateState::default());
        let first = v2s::ToolCallUpdate::new(v2s::ToolCallId::new("t1"))
            .name("grep".to_string())
            .raw_input(serde_json::json!({"pattern": "x"}))
            .status(v2s::ToolCallStatus::InProgress);
        apply_tool_call_update(&emitter, &state, &first);

        let second = v2s::ToolCallUpdate::new(v2s::ToolCallId::new("t1"))
            .status(v2s::ToolCallStatus::Completed)
            .raw_output(serde_json::json!({"ok": true}));
        apply_tool_call_update(&emitter, &state, &second);

        let guard = state.lock();
        let mapping = guard.tools.get("t1").unwrap();
        assert_eq!(mapping.name, "grep");
        assert_eq!(mapping.input, serde_json::json!({"pattern": "x"}));
        assert_eq!(mapping.output, serde_json::json!({"ok": true}));
        assert_eq!(mapping.status, ToolCallStatus::Completed);
    }

    #[test]
    fn terminal_status_cannot_be_overwritten() {
        let emitter = test_emitter("s1");
        let state = Mutex::new(AcpUpdateState::default());
        let completed = v2s::ToolCallUpdate::new(v2s::ToolCallId::new("t1"))
            .status(v2s::ToolCallStatus::Completed);
        apply_tool_call_update(&emitter, &state, &completed);
        let late = v2s::ToolCallUpdate::new(v2s::ToolCallId::new("t1"))
            .status(v2s::ToolCallStatus::InProgress);
        apply_tool_call_update(&emitter, &state, &late);
        let guard = state.lock();
        assert_eq!(
            guard.tools.get("t1").unwrap().status,
            ToolCallStatus::Completed
        );
    }

    #[test]
    fn content_chunks_accumulate_and_emit_live_output() {
        let emitter = test_emitter("s1");
        let mut receiver = emitter.subscribe_for_test();
        let state = Mutex::new(AcpUpdateState::default());
        let first = v2s::ToolCallContentChunk::new(
            v2s::ToolCallId::new("t1"),
            v2s::ToolCallContent::Content(Box::new(v2s::Content::new(v2s::ContentBlock::Text(
                v2s::TextContent::new("line 1\n"),
            )))),
        );
        let second = v2s::ToolCallContentChunk::new(
            v2s::ToolCallId::new("t1"),
            v2s::ToolCallContent::Content(Box::new(v2s::Content::new(v2s::ContentBlock::Text(
                v2s::TextContent::new("line 2\n"),
            )))),
        );
        apply_tool_content_chunk(&emitter, &state, &first);
        apply_tool_content_chunk(&emitter, &state, &second);
        let guard = state.lock();
        assert_eq!(guard.tools.get("t1").unwrap().content, "line 1\nline 2\n");
        drop(guard);
        let events = drain(&mut receiver);
        assert!(events
            .iter()
            .any(|event| event.event_type == AgentEventType::ToolStarted));
    }
}

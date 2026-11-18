//! Shared helpers for ACP event mapping.

use agent_client_protocol::v2::schema as v2s;
use fleet_core_protocol::agent::{
    AgentPlan, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionStatus, StatusChangedPayload,
    ToolCallStatus, TurnStatus,
};
use serde_json::{json, Value};

use crate::event::AgentEventEmitter;

/// Emit a session status change.
pub fn emit_status(emitter: &AgentEventEmitter, status: SessionStatus) {
    emitter.emit(
        fleet_core_protocol::agent::AgentEventType::AgentStatusChanged,
        serde_json::to_value(StatusChangedPayload {
            status,
            reason: None,
        })
        .unwrap_or_default(),
    );
}

/// Emit a raw protocol event for forward compatibility.
pub fn emit_raw<T: serde::Serialize>(emitter: &AgentEventEmitter, kind: &str, value: &T) {
    emitter.emit(
        fleet_core_protocol::agent::AgentEventType::AgentRawEvent,
        json!({
            "source": "acp",
            "kind": kind,
            "raw": serde_json::to_value(value).unwrap_or(Value::Null),
        }),
    );
}

/// Render one v2 content block as text.
pub fn content_block_text(block: &v2s::ContentBlock) -> Option<String> {
    match block {
        v2s::ContentBlock::Text(text) => Some(text.text.clone()),
        v2s::ContentBlock::Image(image) => Some(format!("[image: {}]", image.mime_type.0)),
        v2s::ContentBlock::Audio(audio) => Some(format!("[audio: {}]", audio.mime_type.0)),
        v2s::ContentBlock::ResourceLink(link) => {
            Some(format!("[resource: {}]({})", link.name, link.uri))
        }
        v2s::ContentBlock::Resource(_) => Some("[embedded resource]".to_string()),
        _ => None,
    }
}

/// Render content blocks as text.
pub fn content_blocks_text(blocks: &[v2s::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(content_block_text)
        .collect::<Vec<_>>()
        .join("")
}

/// Render one tool-call content item as text for the unified tool output.
pub fn tool_content_text(content: &v2s::ToolCallContent) -> Option<String> {
    match content {
        v2s::ToolCallContent::Content(content) => content_block_text(&content.content),
        v2s::ToolCallContent::Diff(diff) => Some(format!("[diff: {} changes]", diff.changes.len())),
        v2s::ToolCallContent::Terminal(terminal) => {
            Some(format!("[terminal: {}]", terminal.terminal_id))
        }
        v2s::ToolCallContent::Session(reference) => {
            Some(format!("[session: {}]", reference.session_id))
        }
        v2s::ToolCallContent::Other(other) => serde_json::to_string(other)
            .ok()
            .map(|raw| format!("[{raw}]")),
        _ => None,
    }
}

/// Render a list of tool-call content items.
pub fn tool_contents_text(contents: &[v2s::ToolCallContent]) -> String {
    contents
        .iter()
        .filter_map(tool_content_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map an ACP v2 tool status to the Fleet status.
pub fn tool_status(status: v2s::ToolCallStatus) -> ToolCallStatus {
    match status {
        v2s::ToolCallStatus::Pending => ToolCallStatus::Pending,
        v2s::ToolCallStatus::InProgress => ToolCallStatus::Running,
        v2s::ToolCallStatus::Completed => ToolCallStatus::Completed,
        v2s::ToolCallStatus::Failed => ToolCallStatus::Failed,
        _ => ToolCallStatus::Running,
    }
}

/// Map an ACP v2 plan entry to the Fleet model.
pub fn plan_entry(entry: &v2s::PlanEntry) -> PlanEntry {
    PlanEntry {
        id: None,
        content: entry.content.clone(),
        status: match entry.status {
            v2s::PlanEntryStatus::Pending => PlanEntryStatus::Pending,
            v2s::PlanEntryStatus::InProgress => PlanEntryStatus::InProgress,
            v2s::PlanEntryStatus::Completed => PlanEntryStatus::Completed,
            v2s::PlanEntryStatus::Cancelled => PlanEntryStatus::Cancelled,
            _ => PlanEntryStatus::Unknown,
        },
        priority: match entry.priority {
            v2s::PlanEntryPriority::High => PlanEntryPriority::High,
            v2s::PlanEntryPriority::Medium => PlanEntryPriority::Medium,
            v2s::PlanEntryPriority::Low => PlanEntryPriority::Low,
            _ => PlanEntryPriority::Unknown,
        },
    }
}

/// Build a Fleet plan from mapped entries.
pub fn build_plan(
    session_id: &str,
    turn_id: Option<String>,
    native_plan_id: &str,
    entries: Vec<PlanEntry>,
) -> AgentPlan {
    AgentPlan {
        id: fleet_core_common::new_id("plan"),
        session_id: session_id.to_string(),
        turn_id,
        native_plan_id: Some(native_plan_id.to_string()),
        entries,
        updated_at: fleet_core_common::now(),
    }
}

/// Human-readable name for a v2 stop reason.
pub fn stop_reason_name(reason: &v2s::StopReason) -> String {
    match reason {
        v2s::StopReason::EndTurn => "end_turn".to_string(),
        v2s::StopReason::MaxTokens => "max_tokens".to_string(),
        v2s::StopReason::MaxTurnRequests => "max_turn_requests".to_string(),
        v2s::StopReason::Refusal => "refusal".to_string(),
        v2s::StopReason::Cancelled => "cancelled".to_string(),
        v2s::StopReason::Error(_) => "error".to_string(),
        v2s::StopReason::Other(other) => format!("other:{other:?}"),
        _ => "unknown".to_string(),
    }
}

/// Fleet turn status implied by a v2 stop reason.
pub fn stop_reason_turn_status(reason: &v2s::StopReason) -> TurnStatus {
    match reason {
        v2s::StopReason::Cancelled => TurnStatus::Interrupted,
        v2s::StopReason::Error(_) => TurnStatus::Failed,
        _ => TurnStatus::Completed,
    }
}

/// Human-readable error text for an error stop reason.
pub fn stop_reason_error(reason: &v2s::StopReason) -> Option<String> {
    match reason {
        v2s::StopReason::Error(error) => Some(
            error
                .error
                .as_ref()
                .map(|error| format!("{error:?}"))
                .unwrap_or_else(|| "the agent reported an error".to_string()),
        ),
        _ => None,
    }
}

/// Names of the background work items in a state snapshot.
pub fn background_kinds(background: Option<&[v2s::BackgroundWork]>) -> Vec<String> {
    background
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    v2s::BackgroundWork::Terminal(_) => "terminal".to_string(),
                    v2s::BackgroundWork::ToolCall(_) => "tool_call".to_string(),
                    v2s::BackgroundWork::Subagent(_) => "subagent".to_string(),
                    v2s::BackgroundWork::Other(other) => {
                        format!("other:{}", other.kind)
                    }
                    _ => "other".to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Emit a plan update event.
pub fn emit_plan(emitter: &AgentEventEmitter, plan: &AgentPlan) {
    emitter.emit(
        fleet_core_protocol::agent::AgentEventType::PlanUpdated,
        json!({ "plan": plan }),
    );
}

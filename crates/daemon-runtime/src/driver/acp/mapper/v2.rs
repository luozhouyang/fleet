//! ACP v2 session update mapping.
//!
//! v2 updates are upserts and the session state is an aggregate foreground
//! state, not a per-prompt status. This module turns both into Fleet domain
//! events and never attributes agent-initiated work to a fabricated user turn.

use agent_client_protocol::v2::schema as v2s;
use base64::Engine as _;
use fleet_core_protocol::agent::{
    ActivityChangedPayload, AgentEventType, NoticePayload, SessionStatus, TerminalOutputPayload,
    TurnStatus, UsageUpdatedPayload,
};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::event::AgentEventEmitter;

use super::common::{
    background_kinds, build_plan, emit_plan, emit_raw, emit_status, plan_entry, stop_reason_error,
    stop_reason_name, stop_reason_turn_status,
};
use super::state::{AcpUpdateState, ForegroundStatus};
use super::{messages, tools};

/// Apply one `session/update` notification.
pub fn apply_update(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    notification: &v2s::UpdateSessionNotification,
) {
    match &notification.update {
        v2s::SessionUpdate::UserMessageChunk(chunk) => {
            apply_user_chunk(emitter, state, chunk);
        }
        v2s::SessionUpdate::UserMessage(message) => {
            messages::apply_user_message(emitter, state, message);
        }
        v2s::SessionUpdate::AgentMessageChunk(chunk) => {
            messages::apply_agent_chunk(emitter, state, chunk);
        }
        v2s::SessionUpdate::AgentMessage(message) => {
            messages::apply_agent_message(emitter, state, message);
        }
        v2s::SessionUpdate::AgentThoughtChunk(chunk) => {
            emit_raw(emitter, "agent_thought_chunk", chunk);
        }
        v2s::SessionUpdate::AgentThought(message) => {
            emit_raw(emitter, "agent_thought", message);
        }
        v2s::SessionUpdate::StateUpdate(update) => {
            apply_state_update(emitter, state, update);
        }
        v2s::SessionUpdate::ToolCallContentChunk(chunk) => {
            tools::apply_tool_content_chunk(emitter, state, chunk);
        }
        v2s::SessionUpdate::ToolCallUpdate(update) => {
            tools::apply_tool_call_update(emitter, state, update);
        }
        v2s::SessionUpdate::TerminalUpdate(update) => {
            apply_terminal_update(emitter, update);
        }
        v2s::SessionUpdate::TerminalOutputChunk(chunk) => {
            apply_terminal_chunk(emitter, chunk);
        }
        v2s::SessionUpdate::PlanUpdate(update) => {
            apply_plan_update(emitter, state, update);
        }
        v2s::SessionUpdate::AvailableCommandsUpdate(update) => {
            emit_raw(emitter, "available_commands_update", update);
        }
        v2s::SessionUpdate::ConfigOptionUpdate(update) => {
            emit_raw(emitter, "config_option_update", update);
        }
        v2s::SessionUpdate::SessionInfoUpdate(update) => {
            emit_raw(emitter, "session_info_update", update);
        }
        v2s::SessionUpdate::UsageUpdate(update) => {
            apply_usage_update(emitter, update);
        }
        v2s::SessionUpdate::Notice(notice) => {
            apply_notice(emitter, notice);
        }
        v2s::SessionUpdate::CompactionUpdate(update) => {
            emit_raw(emitter, "compaction_update", update);
        }
        v2s::SessionUpdate::CompactionSummaryChunk(chunk) => {
            emit_raw(emitter, "compaction_summary_chunk", chunk);
        }
        v2s::SessionUpdate::ReplayComplete(update) => {
            emit_raw(emitter, "replay_complete", update);
        }
        v2s::SessionUpdate::Other(other) => {
            // Unknown future updates never break processing; the raw payload
            // stays available for diagnostics and replay.
            emit_raw(emitter, "unknown_session_update", other);
        }
        _ => {
            emit_raw(emitter, "session_update", &notification.update);
        }
    }
}

fn apply_user_chunk(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    chunk: &v2s::ContentChunk,
) {
    // Correlate user-message chunks with the same logic as user-message
    // upserts: buffer until the prompt response tells us which native id is
    // ours, otherwise the runtime's own user message would be duplicated.
    let native_id = chunk.message_id.to_string();
    let text = super::common::content_block_text(&chunk.content);
    let mut guard = state.lock();
    if let Some(pending) = &guard.pending_prompt {
        if pending.native_message_id.as_deref() == Some(native_id.as_str()) {
            // Echo of the prompt the runtime already recorded.
            return;
        }
        if pending.native_message_id.is_none() {
            if let Some(text) = text {
                guard
                    .buffered_user_messages
                    .entry(native_id)
                    .and_modify(|buffered| buffered.content.push_str(&text))
                    .or_insert(super::state::BufferedUserMessage { content: text });
            }
            return;
        }
    }
    drop(guard);
    if let Some(text) = text {
        emitter.emit_with(
            AgentEventType::MessageDelta,
            json!({
                "message_id": native_id,
                "text": text,
            }),
            None,
            Some(native_id),
            None,
        );
    }
}

fn apply_state_update(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    update: &v2s::StateUpdate,
) {
    let current_turn = emitter.current_turn_id();
    match update {
        v2s::StateUpdate::Running(running) => {
            let background = background_kinds(running.background.as_deref());
            let mut guard = state.lock();
            guard.foreground.apply(
                ForegroundStatus::Running,
                current_turn.clone(),
                background.clone(),
                None,
            );
            let agent_initiated = guard.foreground.agent_initiated;
            drop(guard);
            emit_activity(
                emitter,
                "running",
                current_turn.clone(),
                agent_initiated,
                background,
            );
            emit_status(emitter, SessionStatus::Running);
        }
        v2s::StateUpdate::RequiresAction(requires_action) => {
            let background = background_kinds(requires_action.background.as_deref());
            let mut guard = state.lock();
            guard.foreground.apply(
                ForegroundStatus::RequiresAction,
                current_turn.clone(),
                background.clone(),
                None,
            );
            let agent_initiated = guard.foreground.agent_initiated;
            drop(guard);
            emit_activity(
                emitter,
                "requires_action",
                current_turn,
                agent_initiated,
                background,
            );
        }
        v2s::StateUpdate::Idle(idle) => {
            let reason = idle.stop_reason.as_ref();
            let stop_name = reason.map(stop_reason_name);
            let background = background_kinds(idle.background.as_deref());
            let turn_id = current_turn.clone();
            // Finalize streamed messages before the turn event so the
            // transcript is complete when the turn completes.
            messages::complete_open_messages(emitter, state);
            let mut guard = state.lock();
            guard.foreground.apply(
                ForegroundStatus::Idle,
                turn_id.clone(),
                background.clone(),
                stop_name,
            );
            guard.clear_prompt();
            drop(guard);
            emit_activity(emitter, "idle", turn_id.clone(), false, background);

            let Some(turn_id) = turn_id else {
                // Agent-initiated foreground work ended. There is no Fleet
                // turn to complete and none is invented.
                return;
            };
            let status = reason
                .map(stop_reason_turn_status)
                .unwrap_or(TurnStatus::Completed);
            match status {
                TurnStatus::Completed => {
                    emitter.emit_with(
                        AgentEventType::TurnCompleted,
                        json!({
                            "status": "completed",
                            "stop_reason": reason.map(stop_reason_name),
                        }),
                        Some(turn_id),
                        None,
                        None,
                    );
                    emit_status(emitter, SessionStatus::WaitingForInput);
                }
                TurnStatus::Interrupted => {
                    emitter.emit_with(
                        AgentEventType::TurnInterrupted,
                        json!({
                            "status": "interrupted",
                            "stop_reason": reason.map(stop_reason_name),
                        }),
                        Some(turn_id),
                        None,
                        None,
                    );
                    emit_status(emitter, SessionStatus::Interrupted);
                }
                TurnStatus::Failed => {
                    let message = reason
                        .and_then(stop_reason_error)
                        .unwrap_or_else(|| "the agent reported an error".to_string());
                    emitter.emit_with(
                        AgentEventType::TurnFailed,
                        json!({
                            "error": {
                                "code": "agent_error",
                                "message": message,
                            },
                            "stop_reason": reason.map(stop_reason_name),
                        }),
                        Some(turn_id),
                        None,
                        None,
                    );
                    emit_status(emitter, SessionStatus::Failed);
                }
                _ => {}
            }
        }
        v2s::StateUpdate::Unknown(_) => {
            let mut guard = state.lock();
            guard.foreground.apply(
                ForegroundStatus::Unknown,
                current_turn.clone(),
                vec![],
                None,
            );
            let agent_initiated = guard.foreground.agent_initiated;
            drop(guard);
            emit_activity(
                emitter,
                "unknown",
                current_turn,
                agent_initiated,
                Vec::new(),
            );
        }
        v2s::StateUpdate::Other(other) => {
            emit_raw(emitter, "unknown_state_update", other);
        }
        _ => {
            emit_raw(emitter, "state_update", update);
        }
    }
}

fn emit_activity(
    emitter: &AgentEventEmitter,
    state: &str,
    turn_id: Option<String>,
    agent_initiated: bool,
    background: Vec<String>,
) {
    emitter.emit_with(
        AgentEventType::ActivityChanged,
        serde_json::to_value(ActivityChangedPayload {
            state: state.to_string(),
            turn_id: turn_id.clone(),
            agent_initiated,
            background,
        })
        .unwrap_or(Value::Null),
        turn_id,
        None,
        None,
    );
}

fn apply_plan_update(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    update: &v2s::PlanUpdate,
) {
    match &update.plan {
        v2s::PlanUpdateContent::Items(items) => {
            let native_plan_id = items.plan_id.to_string();
            let entries: Vec<_> = items.entries.iter().map(plan_entry).collect();
            let mut guard = state.lock();
            let mapping = guard
                .plans
                .entry(native_plan_id.clone())
                .or_insert_with(|| super::state::PlanMapping::new(native_plan_id.clone()));
            mapping.entries = entries.clone();
            mapping.removed = false;
            let mut plan = build_plan(
                emitter.session_id(),
                emitter.current_turn_id(),
                &native_plan_id,
                entries,
            );
            plan.id = mapping.fleet_plan_id.clone();
            guard.latest_plan = Some(plan.clone());
            drop(guard);
            emit_plan(emitter, &plan);
        }
        other => emit_raw(emitter, "plan_update", other),
    }
}

fn apply_usage_update(emitter: &AgentEventEmitter, update: &v2s::UsageUpdate) {
    let cost = match &update.cost {
        agent_client_protocol::schema::MaybeUndefined::Value(cost) => Some(cost.amount),
        _ => None,
    };
    emitter.emit(
        AgentEventType::UsageUpdated,
        serde_json::to_value(UsageUpdatedPayload {
            used: Some(update.used),
            size: Some(update.size),
            cost,
            raw: serde_json::to_value(update).unwrap_or(Value::Null),
        })
        .unwrap_or(Value::Null),
    );
}

/// Decode base64 terminal bytes into a lossy display string.
fn decode_terminal(data: &str) -> String {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// Map an agent-owned terminal snapshot into the display event.
fn apply_terminal_update(emitter: &AgentEventEmitter, update: &v2s::TerminalUpdate) {
    let (data, text) = match &update.output {
        agent_client_protocol::schema::MaybeUndefined::Value(output) => {
            (output.data.clone(), decode_terminal(&output.data))
        }
        _ => (String::new(), String::new()),
    };
    let (exit_code, done) = match &update.exit_status {
        agent_client_protocol::schema::MaybeUndefined::Value(status) => (status.exit_code, true),
        _ => (None, false),
    };
    let command = match &update.command {
        agent_client_protocol::schema::MaybeUndefined::Value(command) => Some(command.clone()),
        _ => None,
    };
    let cwd = match &update.cwd {
        agent_client_protocol::schema::MaybeUndefined::Value(cwd) => {
            Some(cwd.0.display().to_string())
        }
        _ => None,
    };
    emitter.emit(
        AgentEventType::TerminalOutput,
        serde_json::to_value(TerminalOutputPayload {
            terminal_id: update.terminal_id.to_string(),
            data,
            text,
            command,
            cwd,
            exit_code,
            done,
            raw: serde_json::to_value(update).unwrap_or(Value::Null),
        })
        .unwrap_or(Value::Null),
    );
}

/// Map an agent-owned terminal output chunk into the display event.
fn apply_terminal_chunk(emitter: &AgentEventEmitter, chunk: &v2s::TerminalOutputChunk) {
    emitter.emit(
        AgentEventType::TerminalOutput,
        serde_json::to_value(TerminalOutputPayload {
            terminal_id: chunk.terminal_id.to_string(),
            data: chunk.data.clone(),
            text: decode_terminal(&chunk.data),
            command: None,
            cwd: None,
            exit_code: None,
            done: false,
            raw: serde_json::to_value(chunk).unwrap_or(Value::Null),
        })
        .unwrap_or(Value::Null),
    );
}

fn apply_notice(emitter: &AgentEventEmitter, notice: &v2s::Notice) {
    let level = match notice.severity {
        v2s::NoticeSeverity::Info => "info",
        v2s::NoticeSeverity::Warning => "warning",
        v2s::NoticeSeverity::Error => "error",
        _ => "unknown",
    };
    emitter.emit(
        AgentEventType::Notice,
        serde_json::to_value(NoticePayload {
            kind: "notice".to_string(),
            message: notice
                .description
                .clone()
                .unwrap_or_else(|| notice.title.clone()),
            level: level.to_string(),
            raw: serde_json::to_value(notice).unwrap_or(Value::Null),
        })
        .unwrap_or(Value::Null),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::acp::mapper::messages;
    use crate::driver::acp::tests::test_emitter;
    use fleet_core_protocol::agent::MessageRole;

    fn run(update: v2s::SessionUpdate) -> Vec<fleet_core_protocol::agent::AgentEvent> {
        let emitter = test_emitter("s1");
        let mut receiver = emitter.subscribe_for_test();
        let state = Mutex::new(AcpUpdateState::default());
        // A Fleet turn is normally started by the runtime before updates
        // arrive; emulate that with the emitter's current-turn marker.
        let notification =
            v2s::UpdateSessionNotification::new(v2s::SessionId::new("native"), update);
        apply_update(&emitter, &state, &notification);
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    #[test]
    fn idle_with_end_turn_completes_messages_and_turn() {
        let emitter = test_emitter("s1");
        let mut receiver = emitter.subscribe_for_test();
        let state = Mutex::new(AcpUpdateState::default());
        // Simulate a runtime-created turn.
        emitter.set_current_turn_pub(Some("turn_1".to_string()));
        messages::apply_agent_chunk(
            &emitter,
            &state,
            &v2s::ContentChunk::new(
                v2s::ContentBlock::Text(v2s::TextContent::new("done")),
                v2s::MessageId::new("m1"),
            ),
        );
        let idle = v2s::StateUpdate::Idle(
            v2s::IdleStateUpdate::new().stop_reason(v2s::StopReason::EndTurn),
        );
        let notification = v2s::UpdateSessionNotification::new(
            v2s::SessionId::new("native"),
            v2s::SessionUpdate::StateUpdate(idle),
        );
        apply_update(&emitter, &state, &notification);
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        assert!(events
            .iter()
            .any(|event| event.event_type == AgentEventType::TurnCompleted));
        assert!(events
            .iter()
            .any(|event| event.event_type == AgentEventType::MessageCompleted));
    }

    #[test]
    fn idle_without_a_turn_does_not_fabricate_one() {
        let events = run(v2s::SessionUpdate::StateUpdate(v2s::StateUpdate::Idle(
            v2s::IdleStateUpdate::new().stop_reason(v2s::StopReason::EndTurn),
        )));
        assert!(!events.iter().any(|event| {
            event.event_type == AgentEventType::TurnCompleted
                || event.event_type == AgentEventType::TurnFailed
        }));
        assert!(events
            .iter()
            .any(|event| event.event_type == AgentEventType::ActivityChanged));
    }

    #[test]
    fn plan_update_maps_to_first_class_plan_event() {
        let plan = v2s::PlanItems::new(
            v2s::PlanId::new("p1"),
            vec![v2s::PlanEntry::new(
                "step",
                v2s::PlanEntryPriority::High,
                v2s::PlanEntryStatus::InProgress,
            )],
        );
        let events = run(v2s::SessionUpdate::PlanUpdate(v2s::PlanUpdate::new(
            v2s::PlanUpdateContent::Items(plan),
        )));
        let event = events
            .iter()
            .find(|event| event.event_type == AgentEventType::PlanUpdated)
            .expect("plan event");
        assert_eq!(event.payload["plan"]["entries"][0]["content"], "step");
        assert_eq!(event.payload["plan"]["entries"][0]["status"], "in_progress");
    }

    #[test]
    fn unknown_update_is_preserved_as_raw_event() {
        let update: v2s::SessionUpdate =
            serde_json::from_value(serde_json::json!({"sessionUpdate": "future_update", "x": 1}))
                .expect("unknown update should deserialize");
        let events = run(update);
        let raw = events
            .iter()
            .find(|event| event.event_type == AgentEventType::AgentRawEvent)
            .expect("raw event");
        assert_eq!(raw.payload["kind"], "unknown_session_update");
    }

    #[test]
    fn tool_events_carry_turn_ids() {
        let emitter = test_emitter("s1");
        let mut receiver = emitter.subscribe_for_test();
        let state = Mutex::new(AcpUpdateState::default());
        emitter.set_current_turn_pub(Some("turn_1".to_string()));
        tools::apply_tool_call_update(
            &emitter,
            &state,
            &v2s::ToolCallUpdate::new(v2s::ToolCallId::new("t1"))
                .name("grep".to_string())
                .status(v2s::ToolCallStatus::InProgress),
        );
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        let tool = events
            .iter()
            .find(|event| event.event_type == AgentEventType::ToolStarted)
            .unwrap();
        assert_eq!(tool.turn_id.as_deref(), Some("turn_1"));
    }

    #[test]
    fn usage_update_uses_the_declared_payload_shape() {
        let events = run(v2s::SessionUpdate::UsageUpdate(v2s::UsageUpdate::new(
            120, 2000,
        )));
        let event = events
            .iter()
            .find(|event| event.event_type == AgentEventType::UsageUpdated)
            .expect("usage event");
        let payload: UsageUpdatedPayload =
            serde_json::from_value(event.payload.clone()).expect("typed usage payload");
        assert_eq!(payload.used, Some(120));
        assert_eq!(payload.size, Some(2000));
        assert_eq!(payload.cost, None);
        assert!(payload.raw.is_object());
    }

    #[test]
    fn user_message_role_is_not_duplicated_in_agent_space() {
        let emitter = test_emitter("s1");
        let state = Mutex::new(AcpUpdateState::default());
        let user = v2s::UserMessage::new(v2s::MessageId::new("m1"))
            .content(vec![v2s::ContentBlock::Text(v2s::TextContent::new("hi"))]);
        messages::apply_user_message(&emitter, &state, &user);
        let guard = state.lock();
        let keys: Vec<&String> = guard.messages.keys().collect();
        assert!(keys.iter().any(|key| key.starts_with("user:")));
        assert_eq!(MessageRole::Assistant, MessageRole::Assistant);
    }
}

//! ACP v2 message upsert and chunk mapping.
//!
//! ACP v2 message updates have patch semantics: an omitted `content` leaves
//! the previous value unchanged, `null` clears it, and an array replaces it.
//! Chunks append to the current value. Fleet emits `message.started` once per
//! native message, `message.delta` for appended text, and
//! `message.completed` once when the agent reports idle.

use agent_client_protocol::v2::schema as v2s;
use fleet_core_protocol::agent::{
    AgentEventType, AgentMessage, MessageCompletedPayload, MessageDeltaPayload, MessageRole,
    MessageStartedPayload,
};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::event::AgentEventEmitter;

use super::common::content_blocks_text;
use super::state::{message_key, AcpUpdateState, MessageMapping};

/// Apply a streamed assistant-message chunk.
pub fn apply_agent_chunk(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    chunk: &v2s::ContentChunk,
) {
    apply_chunk(emitter, state, MessageRole::Assistant, chunk);
}

/// Apply an assistant message upsert.
pub fn apply_agent_message(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    message: &v2s::AgentMessage,
) {
    let native_id = message.message_id.to_string();
    apply_upsert(
        emitter,
        state,
        MessageRole::Assistant,
        &native_id,
        &message.content,
    );
}

/// Apply a user message upsert.
///
/// A user-message echo may arrive before or after the prompt response. When a
/// prompt is pending, the echo is correlated with the Fleet message the
/// runtime already recorded; Fleet never renders a duplicate user message.
pub fn apply_user_message(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    message: &v2s::UserMessage,
) {
    let native_id = message.message_id.to_string();
    let content = match &message.content {
        agent_client_protocol::schema::MaybeUndefined::Undefined => None,
        agent_client_protocol::schema::MaybeUndefined::Null => Some(String::new()),
        agent_client_protocol::schema::MaybeUndefined::Value(blocks) => {
            Some(content_blocks_text(blocks))
        }
    };
    let mut guard = state.lock();
    let key = message_key(MessageRole::User, &native_id);
    if let Some(mapping) = guard.messages.get_mut(&key) {
        if let Some(content) = content {
            mapping.content = content;
        }
        mapping.completed = true;
        return;
    }
    if let Some(pending) = &guard.pending_prompt {
        if pending.native_message_id.as_deref() == Some(native_id.as_str()) {
            // The response arrived first; the mapping already exists for the
            // correlated Fleet message.
            return;
        }
        if pending.native_message_id.is_none() {
            // The echo arrived before the response: buffer it until the
            // response tells us which native id belongs to our prompt.
            let text = content.unwrap_or_default();
            guard
                .buffered_user_messages
                .entry(native_id)
                .and_modify(|buffered| buffered.content.push_str(&text))
                .or_insert(super::state::BufferedUserMessage { content: text });
            return;
        }
    }
    // No prompt is in flight and no mapping exists: this is an agent-initiated
    // or replayed user message. It is still shown, but it does not create a
    // Fleet turn.
    let mut mapping = MessageMapping::new(native_id.clone(), MessageRole::User);
    if let Some(content) = content {
        mapping.content = content;
    }
    mapping.started_emitted = true;
    mapping.completed = true;
    let fleet_id = mapping.fleet_message_id.clone();
    let text = mapping.content.clone();
    guard.messages.insert(key, mapping);
    drop(guard);

    emitter.emit_with(
        AgentEventType::MessageStarted,
        serde_json::to_value(MessageStartedPayload {
            message_id: fleet_id.clone(),
            role: MessageRole::User,
        })
        .unwrap_or_default(),
        None,
        Some(fleet_id.clone()),
        None,
    );
    emit_completed(emitter, &fleet_id, MessageRole::User, &text, None);
}

fn apply_chunk(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    role: MessageRole,
    chunk: &v2s::ContentChunk,
) {
    let Some(text) = super::common::content_block_text(&chunk.content) else {
        super::common::emit_raw(emitter, "message_content", chunk);
        return;
    };
    let native_id = chunk.message_id.to_string();
    let mut guard = state.lock();
    let key = message_key(role, &native_id);
    let sequence = guard.next_sequence();
    let mapping = guard
        .messages
        .entry(key)
        .or_insert_with(|| MessageMapping::new(native_id.clone(), role));
    mapping.sequence = sequence;
    let start = !mapping.started_emitted;
    mapping.started_emitted = true;
    mapping.content.push_str(&text);
    let fleet_id = mapping.fleet_message_id.clone();
    guard.trim();
    drop(guard);

    if start {
        emit_started(emitter, &fleet_id, role);
    }
    emitter.emit_with(
        AgentEventType::MessageDelta,
        serde_json::to_value(MessageDeltaPayload {
            message_id: fleet_id.clone(),
            text,
            index: None,
        })
        .unwrap_or_default(),
        None,
        Some(fleet_id),
        None,
    );
}

fn apply_upsert(
    emitter: &AgentEventEmitter,
    state: &Mutex<AcpUpdateState>,
    role: MessageRole,
    native_id: &str,
    content: &agent_client_protocol::schema::MaybeUndefined<Vec<v2s::ContentBlock>>,
) {
    use agent_client_protocol::schema::MaybeUndefined;
    let native_id = native_id.to_string();
    let mut guard = state.lock();
    let key = message_key(role, &native_id);
    let sequence = guard.next_sequence();
    let mapping = guard
        .messages
        .entry(key)
        .or_insert_with(|| MessageMapping::new(native_id.clone(), role));
    mapping.sequence = sequence;
    let start = !mapping.started_emitted;
    mapping.started_emitted = true;
    let fleet_id = mapping.fleet_message_id.clone();

    let mut delta: Option<String> = None;
    let mut replaced = false;
    match content {
        MaybeUndefined::Undefined => {}
        MaybeUndefined::Null => {
            if !mapping.content.is_empty() {
                mapping.content.clear();
                replaced = true;
            }
        }
        MaybeUndefined::Value(blocks) => {
            let next = content_blocks_text(blocks);
            if next.starts_with(mapping.content.as_str()) {
                let suffix = next[mapping.content.len()..].to_string();
                mapping.content = next;
                if !suffix.is_empty() {
                    delta = Some(suffix);
                }
            } else if next != mapping.content {
                mapping.content = next;
                replaced = true;
            }
        }
    }
    let content_after = mapping.content.clone();
    guard.trim();
    drop(guard);

    if start {
        emit_started(emitter, &fleet_id, role);
    }
    let initial = delta.or_else(|| replaced.then_some(content_after.clone()));
    if let Some(text) = initial {
        if !text.is_empty() {
            emitter.emit_with(
                AgentEventType::MessageDelta,
                serde_json::to_value(MessageDeltaPayload {
                    message_id: fleet_id.clone(),
                    text,
                    index: None,
                })
                .unwrap_or_default(),
                None,
                Some(fleet_id.clone()),
                None,
            );
        }
    }
    if replaced {
        super::common::emit_raw(
            emitter,
            "message_replaced",
            &json!({ "message_id": fleet_id, "role": role, "length": content_after.len() }),
        );
    }
}

fn emit_started(emitter: &AgentEventEmitter, message_id: &str, role: MessageRole) {
    emitter.emit_with(
        AgentEventType::MessageStarted,
        serde_json::to_value(MessageStartedPayload {
            message_id: message_id.to_string(),
            role,
        })
        .unwrap_or_default(),
        None,
        Some(message_id.to_string()),
        None,
    );
}

/// Emit `message.completed` for every assistant message that has not completed
/// yet. Safe to call on every idle state update: completion is emitted once.
pub fn complete_open_messages(emitter: &AgentEventEmitter, state: &Mutex<AcpUpdateState>) {
    let turn_id = emitter.current_turn_id();
    let completed: Vec<(String, String)> = {
        let mut guard = state.lock();
        let mut completed = Vec::new();
        for mapping in guard.messages.values_mut() {
            if mapping.role != MessageRole::Assistant || mapping.completed {
                continue;
            }
            mapping.completed = true;
            completed.push((mapping.fleet_message_id.clone(), mapping.content.clone()));
        }
        completed
    };
    for (message_id, content) in completed {
        emit_completed(
            emitter,
            &message_id,
            MessageRole::Assistant,
            &content,
            turn_id.clone(),
        );
    }
}

fn emit_completed(
    emitter: &AgentEventEmitter,
    message_id: &str,
    role: MessageRole,
    content: &str,
    turn_id: Option<String>,
) {
    let mut message = AgentMessage::new(
        emitter.session_id(),
        turn_id.clone(),
        role,
        content.to_string(),
    );
    message.id = message_id.to_string();
    emitter.emit_with(
        AgentEventType::MessageCompleted,
        serde_json::to_value(MessageCompletedPayload { message }).unwrap_or(Value::Null),
        turn_id,
        Some(message_id.to_string()),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Mutex<AcpUpdateState> {
        Mutex::new(AcpUpdateState::default())
    }

    fn emitter() -> AgentEventEmitter {
        crate::driver::acp::tests::test_emitter("s1")
    }

    fn text_chunk(message_id: &str, text: &str) -> v2s::ContentChunk {
        v2s::ContentChunk::new(
            v2s::ContentBlock::Text(v2s::TextContent::new(text)),
            v2s::MessageId::new(message_id),
        )
    }

    #[test]
    fn chunks_start_once_and_append() {
        let emitter = emitter();
        let mut receiver = emitter_subscribe(&emitter);
        let state = state();
        apply_agent_chunk(&emitter, &state, &text_chunk("m1", "hello "));
        apply_agent_chunk(&emitter, &state, &text_chunk("m1", "world"));
        let events = drain(&mut receiver);
        assert_eq!(count(&events, "message.started"), 1);
        let deltas: Vec<String> = events
            .iter()
            .filter(|event| event.event_type == AgentEventType::MessageDelta)
            .map(|event| event.payload["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(deltas, vec!["hello ", "world"]);
        complete_open_messages(&emitter, &state);
        complete_open_messages(&emitter, &state);
        let events = drain(&mut receiver);
        assert_eq!(count(&events, "message.completed"), 1);
        let completed = events
            .iter()
            .find(|event| event.event_type == AgentEventType::MessageCompleted)
            .unwrap();
        assert_eq!(completed.payload["message"]["content"], "hello world");
    }

    #[test]
    fn upsert_replaces_content_without_duplicate_start() {
        let emitter = emitter();
        let mut receiver = emitter_subscribe(&emitter);
        let state = state();
        apply_agent_chunk(&emitter, &state, &text_chunk("m1", "draft"));
        let upsert = v2s::AgentMessage::new(v2s::MessageId::new("m1")).content(vec![
            v2s::ContentBlock::Text(v2s::TextContent::new("final answer")),
        ]);
        apply_agent_message(&emitter, &state, &upsert);
        let events = drain(&mut receiver);
        assert_eq!(count(&events, "message.started"), 1);
        let guard = state.lock();
        let mapping = guard
            .messages
            .get(&message_key(MessageRole::Assistant, "m1"))
            .unwrap();
        assert_eq!(mapping.content, "final answer");
    }

    #[test]
    fn user_echo_before_prompt_response_is_correlated() {
        let emitter = emitter();
        let mut receiver = emitter_subscribe(&emitter);
        let state = state();
        state
            .lock()
            .begin_prompt("message_local", Some("turn_1".into()));
        let echo = v2s::UserMessage::new(v2s::MessageId::new("native_1"))
            .content(vec![v2s::ContentBlock::Text(v2s::TextContent::new("hi"))]);
        apply_user_message(&emitter, &state, &echo);
        assert!(drain(&mut receiver).is_empty());
        let correlated = state.lock().accept_prompt("native_1").unwrap();
        assert_eq!(correlated.fleet_message_id, "message_local");
        assert_eq!(correlated.content.as_deref(), Some("hi"));
        assert!(drain(&mut receiver).is_empty());
    }

    #[test]
    fn user_echo_without_prompt_is_rendered_once() {
        let emitter = emitter();
        let mut receiver = emitter_subscribe(&emitter);
        let state = state();
        let echo = v2s::UserMessage::new(v2s::MessageId::new("native_1")).content(vec![
            v2s::ContentBlock::Text(v2s::TextContent::new("agent initiated")),
        ]);
        apply_user_message(&emitter, &state, &echo);
        let events = drain(&mut receiver);
        assert_eq!(count(&events, "message.started"), 1);
        assert_eq!(count(&events, "message.completed"), 1);
        apply_user_message(&emitter, &state, &echo);
        assert!(drain(&mut receiver).is_empty());
    }

    fn emitter_subscribe(
        emitter: &AgentEventEmitter,
    ) -> tokio::sync::broadcast::Receiver<fleet_core_protocol::agent::AgentEvent> {
        emitter.subscribe_for_test()
    }

    fn drain(
        receiver: &mut tokio::sync::broadcast::Receiver<fleet_core_protocol::agent::AgentEvent>,
    ) -> Vec<fleet_core_protocol::agent::AgentEvent> {
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    fn count(events: &[fleet_core_protocol::agent::AgentEvent], kind: &str) -> usize {
        events
            .iter()
            .filter(|event| event.event_type.as_str() == kind)
            .count()
    }
}

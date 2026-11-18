//! Pure mappings between ACP protocol messages and unified agent events.
//!
//! The adapter owns process and connection state; everything in this module is
//! a deterministic transformation so it can be unit tested without an ACP
//! agent. AskUserQuestion-style elicitation forms use the field naming
//! convention `question_<n>` plus an optional `question_<n>_custom` free-form
//! field.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use fleet_core_common::{new_id, now};
use fleet_core_protocol::agent::{
    AgentEventType, AgentMessage, AgentPermissionOption, AgentPermissionRequest, AgentQuestion,
    AgentQuestionOption, AgentQuestionRequest, AgentToolCall, MessageCompletedPayload,
    MessageDeltaPayload, MessageRole, MessageStartedPayload, PermissionDecision, PermissionStatus,
    QuestionDecision, QuestionStatus, SessionStatus, StatusChangedPayload, ToolEventPayload,
};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::error::RuntimeError;
use crate::event::{AgentEventEmitter, EventDeduplicator};

/// Number of native message ids remembered for duplicate-start protection.
const SEEN_MESSAGE_CAPACITY: usize = 5_000;

use crate::acp::protocol::{
    ContentBlock, CreateElicitationRequest, CreateElicitationResponse, ElicitationAcceptAction,
    ElicitationAction, ElicitationContentValue, ElicitationMode, ElicitationScope,
    MultiSelectItems, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    ToolCall, ToolCallContent, ToolCallStatus as AcpToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields,
};
use fleet_core_protocol::agent::ToolCallStatus;

/// Per-session streaming state used to deduplicate message starts and remember
/// tool calls across incremental `tool_call_update` notifications.
pub struct SessionUpdateState {
    /// Bounded native-message-id set so a replayed chunk does not emit a
    /// second `message.started` or allocate a duplicate message record.
    seen_messages: EventDeduplicator,
    open_messages: BTreeMap<String, String>,
    tools: HashMap<String, AgentToolCall>,
}

impl Default for SessionUpdateState {
    fn default() -> Self {
        Self {
            seen_messages: EventDeduplicator::new(SEEN_MESSAGE_CAPACITY),
            open_messages: BTreeMap::new(),
            tools: HashMap::new(),
        }
    }
}

/// Map an ACP permission request into the unified model.
///
/// `session_id` is the ACP-native session id; callers that already know the
/// unified session id should overwrite it before emitting the request.
pub fn map_permission_request(request: &RequestPermissionRequest) -> AgentPermissionRequest {
    let tool = &request.tool_call;
    let title = tool.fields.title.clone();
    let tool_name = tool
        .fields
        .name
        .clone()
        .or_else(|| title.clone())
        .unwrap_or_else(|| "tool".to_string());
    let description =
        title.unwrap_or_else(|| "The agent needs permission to continue.".to_string());
    let input = tool
        .fields
        .raw_input
        .clone()
        .or_else(|| serde_json::to_value(tool).ok())
        .unwrap_or(Value::Null);

    AgentPermissionRequest {
        id: new_id("permission"),
        session_id: request.session_id.to_string(),
        turn_id: None,
        tool_call_id: Some(tool.tool_call_id.to_string()),
        tool_name,
        description,
        input,
        options: request
            .options
            .iter()
            .map(|option| AgentPermissionOption {
                option_id: option.option_id.to_string(),
                name: option.name.clone(),
                kind: permission_kind(option.kind),
            })
            .collect(),
        status: PermissionStatus::Pending,
        created_at: now(),
        resolved_at: None,
        expires_at: None,
    }
}

/// Map a runtime permission decision back into an ACP response.
///
/// ACP resolves permissions by selecting one of the options the agent offered;
/// there is no separate boolean. The caller must pass the original request so
/// the fallback can choose an `allow_*` or `reject_*` option.
pub fn map_permission_response(
    request: &AgentPermissionRequest,
    decision: &PermissionDecision,
) -> RequestPermissionResponse {
    if let Some(option_id) = &decision.option_id {
        return RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option_id.clone()),
        ));
    }
    let fallback = if decision.approve {
        request
            .options
            .iter()
            .find(|option| option.kind.starts_with("allow"))
    } else {
        request
            .options
            .iter()
            .find(|option| option.kind.starts_with("reject"))
    };
    match fallback {
        Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option.option_id.clone()),
        )),
        None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
    }
}

/// Map an ACP form elicitation request into a unified question request.
///
/// Only `form` elicitations carrying at least one schema property are
/// supported; URL elicitations and empty schemas are reported as unsupported.
/// `session_id` is left empty so the event emitter can fill in the unified
/// session id.
pub fn map_elicitation_request(
    request: &CreateElicitationRequest,
) -> Result<AgentQuestionRequest, RuntimeError> {
    let form = match &request.mode {
        ElicitationMode::Form(form) => form,
        _ => {
            return Err(RuntimeError::Unsupported(
                "only form elicitations can be presented as user questions".to_string(),
            ))
        }
    };

    let properties = &form.requested_schema.properties;
    let keys: BTreeSet<&String> = properties.keys().collect();
    let custom_fields: BTreeSet<String> = keys
        .iter()
        .filter_map(|id| {
            id.strip_suffix("_custom")
                .filter(|base| keys.contains(&base.to_string()))
                .map(|_| (*id).clone())
        })
        .collect();

    let mut questions = Vec::new();
    for (id, property) in properties {
        if custom_fields.contains(id) {
            continue;
        }
        let allow_custom = custom_fields.contains(&format!("{id}_custom"));
        questions.push(map_question(id, property, allow_custom));
    }
    if questions.is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "elicitation schema does not contain any question fields".to_string(),
        ));
    }
    questions.sort_by_key(|question| question_sort_key(&question.id));

    let tool_call_id = match &form.scope {
        ElicitationScope::Session(scope) => scope.tool_call_id.as_ref().map(ToString::to_string),
        _ => None,
    };

    Ok(AgentQuestionRequest {
        id: new_id("question"),
        session_id: String::new(),
        turn_id: None,
        tool_call_id,
        questions,
        answers: Vec::new(),
        status: QuestionStatus::Pending,
        created_at: now(),
        resolved_at: None,
        expires_at: None,
    })
}

/// Map a runtime question decision into an ACP elicitation response.
pub fn map_elicitation_response(
    request: &AgentQuestionRequest,
    decision: &QuestionDecision,
) -> CreateElicitationResponse {
    if decision.reject {
        return CreateElicitationResponse::new(ElicitationAction::Decline);
    }
    let mut content = BTreeMap::new();
    for question in &request.questions {
        let Some(answer) = decision
            .answers
            .iter()
            .find(|answer| answer.id == question.id)
        else {
            continue;
        };
        let wire_values: Vec<String> = answer
            .selected
            .iter()
            .map(|selected| {
                question
                    .options
                    .iter()
                    .find(|option| &option.label == selected)
                    .and_then(|option| option.value.clone())
                    .unwrap_or_else(|| selected.clone())
            })
            .collect();
        if question.multi_select {
            content.insert(
                question.id.clone(),
                ElicitationContentValue::StringArray(wire_values),
            );
        } else if let Some(selected) = wire_values.first() {
            content.insert(
                question.id.clone(),
                ElicitationContentValue::String(selected.clone()),
            );
        }
        if let Some(custom) = &answer.custom {
            content.insert(
                format!("{}_custom", question.id),
                ElicitationContentValue::String(custom.clone()),
            );
        }
    }
    CreateElicitationResponse::new(ElicitationAction::Accept(
        ElicitationAcceptAction::new().content(Some(content)),
    ))
}

/// Apply one `session/update` notification to the unified event stream.
pub fn apply_session_update(
    emitter: &AgentEventEmitter,
    state: &Mutex<SessionUpdateState>,
    notification: &SessionNotification,
) {
    match &notification.update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            apply_message_chunk(emitter, state, chunk);
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            emit_raw(emitter, "agent_thought_chunk", chunk);
        }
        SessionUpdate::ToolCall(call) => {
            apply_tool_call(emitter, state, call);
        }
        SessionUpdate::ToolCallUpdate(update) => {
            apply_tool_call_update(emitter, state, update);
        }
        other => {
            emit_raw(emitter, "session_update", other);
        }
    }
}

/// Emit `message.completed` for every assistant message streamed during the
/// turn and clear the open-message table.
pub fn complete_open_messages(emitter: &AgentEventEmitter, state: &Mutex<SessionUpdateState>) {
    let open = {
        let mut guard = state.lock();
        std::mem::take(&mut guard.open_messages)
    };
    let turn_id = emitter.current_turn_id();
    for (message_id, text) in open {
        let mut message = AgentMessage::new(
            emitter.session_id(),
            turn_id.clone(),
            MessageRole::Assistant,
            text,
        );
        message.id = message_id.clone();
        emitter.emit_with(
            AgentEventType::MessageCompleted,
            serde_json::to_value(MessageCompletedPayload { message }).unwrap_or_default(),
            turn_id.clone(),
            Some(message_id),
            None,
        );
    }
}

/// Emit a status change through the unified event stream.
pub fn emit_status(emitter: &AgentEventEmitter, status: SessionStatus) {
    emitter.emit(
        AgentEventType::AgentStatusChanged,
        serde_json::to_value(StatusChangedPayload {
            status,
            reason: None,
        })
        .unwrap_or_default(),
    );
}

fn apply_message_chunk(
    emitter: &AgentEventEmitter,
    state: &Mutex<SessionUpdateState>,
    chunk: &crate::acp::protocol::ContentChunk,
) {
    let ContentBlock::Text(text) = &chunk.content else {
        emit_raw(emitter, "agent_message_chunk", chunk);
        return;
    };
    let message_id = chunk
        .message_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "assistant".to_string());

    let started = {
        let mut guard = state.lock();
        let started = guard.seen_messages.first_seen("message", &message_id);
        guard
            .open_messages
            .entry(message_id.clone())
            .or_default()
            .push_str(&text.text);
        started
    };

    if started {
        emitter.emit_with(
            AgentEventType::MessageStarted,
            serde_json::to_value(MessageStartedPayload {
                message_id: message_id.clone(),
                role: MessageRole::Assistant,
            })
            .unwrap_or_default(),
            None,
            Some(message_id.clone()),
            None,
        );
    }
    emitter.emit_with(
        AgentEventType::MessageDelta,
        serde_json::to_value(MessageDeltaPayload {
            message_id: message_id.clone(),
            text: text.text.clone(),
            index: None,
        })
        .unwrap_or_default(),
        None,
        Some(message_id),
        None,
    );
}

fn apply_tool_call(
    emitter: &AgentEventEmitter,
    state: &Mutex<SessionUpdateState>,
    call: &ToolCall,
) {
    let status = tool_status(call.status);
    let tool = AgentToolCall {
        id: call.tool_call_id.to_string(),
        session_id: emitter.session_id().to_string(),
        turn_id: emitter.current_turn_id(),
        name: call.name.clone().unwrap_or_else(|| call.title.clone()),
        status,
        input: call.raw_input.clone().unwrap_or(Value::Null),
        output: call
            .raw_output
            .clone()
            .or_else(|| serde_json::to_value(&call.content).ok())
            .filter(|value| !value.is_null())
            .unwrap_or(Value::Null),
        error: None,
        started_at: now(),
        completed_at: matches!(
            status,
            ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Cancelled
        )
        .then(now),
    };
    {
        let mut guard = state.lock();
        guard.tools.insert(tool.id.clone(), tool.clone());
    }
    emit_tool(emitter, &tool);
}

fn apply_tool_call_update(
    emitter: &AgentEventEmitter,
    state: &Mutex<SessionUpdateState>,
    update: &ToolCallUpdate,
) {
    let tool = {
        let mut guard = state.lock();
        let id = update.tool_call_id.to_string();
        let mut tool = guard
            .tools
            .get(&id)
            .cloned()
            .unwrap_or_else(|| AgentToolCall {
                id: id.clone(),
                session_id: emitter.session_id().to_string(),
                turn_id: emitter.current_turn_id(),
                name: "tool".to_string(),
                status: ToolCallStatus::Running,
                input: Value::Null,
                output: Value::Null,
                error: None,
                started_at: now(),
                completed_at: None,
            });
        apply_tool_fields(&mut tool, &update.fields);
        guard.tools.insert(id, tool.clone());
        tool
    };
    emit_tool(emitter, &tool);
}

fn apply_tool_fields(tool: &mut AgentToolCall, fields: &ToolCallUpdateFields) {
    if let Some(name) = &fields.name {
        tool.name = name.clone();
    } else if let Some(title) = &fields.title {
        tool.name = title.clone();
    }
    if let Some(input) = &fields.raw_input {
        tool.input = input.clone();
    }
    if let Some(content) = &fields.content {
        if let Some(text) = tool_content_text(content) {
            if tool.status == ToolCallStatus::Failed {
                tool.error = Some(text.clone());
            }
            tool.output = Value::String(text);
        } else {
            tool.output = serde_json::to_value(content).unwrap_or(Value::Null);
        }
    }
    if let Some(output) = &fields.raw_output {
        tool.output = output.clone();
    }
    if let Some(status) = fields.status {
        tool.status = tool_status(status);
        if matches!(
            tool.status,
            ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Cancelled
        ) {
            tool.completed_at.get_or_insert_with(now);
        }
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

fn emit_raw<T: serde::Serialize>(emitter: &AgentEventEmitter, kind: &str, value: &T) {
    emitter.emit(
        AgentEventType::AgentRawEvent,
        json!({
            "source": "acp",
            "kind": kind,
            "raw": serde_json::to_value(value).unwrap_or(Value::Null),
        }),
    );
}

fn map_question(
    id: &str,
    property: &crate::acp::protocol::ElicitationPropertySchema,
    allow_custom: bool,
) -> AgentQuestion {
    use agent_client_protocol::v1::schema::ElicitationPropertySchema;
    match property {
        ElicitationPropertySchema::String(schema) => {
            let options = if let Some(options) = &schema.one_of {
                options
                    .iter()
                    .map(|option| AgentQuestionOption {
                        label: option.title.clone(),
                        description: option.description.clone(),
                        value: Some(option.value.clone()),
                    })
                    .collect()
            } else {
                schema
                    .enum_values
                    .as_ref()
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| AgentQuestionOption {
                                label: value.clone(),
                                description: None,
                                value: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            AgentQuestion {
                id: id.to_string(),
                header: schema.title.clone(),
                question: schema
                    .description
                    .clone()
                    .or_else(|| schema.title.clone())
                    .unwrap_or_else(|| id.to_string()),
                options,
                multi_select: false,
                allow_custom,
                is_secret: false,
            }
        }
        ElicitationPropertySchema::Array(schema) => {
            let options = match &schema.items {
                MultiSelectItems::String(items) => items
                    .values
                    .iter()
                    .map(|value| AgentQuestionOption {
                        label: value.clone(),
                        description: None,
                        value: None,
                    })
                    .collect(),
                MultiSelectItems::Titled(items) => items
                    .options
                    .iter()
                    .map(|option| AgentQuestionOption {
                        label: option.title.clone(),
                        description: option.description.clone(),
                        value: Some(option.value.clone()),
                    })
                    .collect(),
                _ => Vec::new(),
            };
            AgentQuestion {
                id: id.to_string(),
                header: schema.title.clone(),
                question: schema
                    .description
                    .clone()
                    .or_else(|| schema.title.clone())
                    .unwrap_or_else(|| id.to_string()),
                options,
                multi_select: true,
                allow_custom,
                is_secret: false,
            }
        }
        ElicitationPropertySchema::Number(schema) => free_form_question(
            id,
            schema.title.clone(),
            schema.description.clone(),
            allow_custom,
        ),
        ElicitationPropertySchema::Integer(schema) => free_form_question(
            id,
            schema.title.clone(),
            schema.description.clone(),
            allow_custom,
        ),
        ElicitationPropertySchema::Boolean(schema) => free_form_question(
            id,
            schema.title.clone(),
            schema.description.clone(),
            allow_custom,
        ),
        _ => free_form_question(id, None, None, true),
    }
}

fn free_form_question(
    id: &str,
    title: Option<String>,
    description: Option<String>,
    allow_custom: bool,
) -> AgentQuestion {
    AgentQuestion {
        id: id.to_string(),
        header: title.clone(),
        question: description
            .or_else(|| title.clone())
            .unwrap_or_else(|| id.to_string()),
        options: Vec::new(),
        multi_select: false,
        allow_custom: allow_custom || title.is_none(),
        is_secret: false,
    }
}

fn tool_content_text(content: &[ToolCallContent]) -> Option<String> {
    let mut parts = Vec::new();
    for item in content {
        if let ToolCallContent::Content(block) = item {
            if let ContentBlock::Text(text) = &block.content {
                parts.push(text.text.clone());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn tool_status(status: AcpToolCallStatus) -> ToolCallStatus {
    match status {
        AcpToolCallStatus::Pending => ToolCallStatus::Pending,
        AcpToolCallStatus::InProgress => ToolCallStatus::Running,
        AcpToolCallStatus::Completed => ToolCallStatus::Completed,
        AcpToolCallStatus::Failed => ToolCallStatus::Failed,
        _ => ToolCallStatus::Running,
    }
}

fn permission_kind(kind: PermissionOptionKind) -> String {
    match kind {
        PermissionOptionKind::AllowOnce => "allow_once",
        PermissionOptionKind::AllowAlways => "allow_always",
        PermissionOptionKind::RejectOnce => "reject_once",
        PermissionOptionKind::RejectAlways => "reject_always",
        _ => "unknown",
    }
    .to_string()
}

fn question_sort_key(id: &str) -> (u64, String) {
    if let Some(rest) = id.strip_prefix("question_") {
        if let Some((index, tail)) = rest.split_once('_') {
            if let Ok(index) = index.parse::<u64>() {
                return (index, tail.to_string());
            }
        } else if let Ok(index) = rest.parse::<u64>() {
            return (index, String::new());
        }
    }
    (u64::MAX, id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::protocol::{ContentChunk, TextContent};
    use crate::driver::acp::tests::test_emitter;

    #[test]
    fn replayed_chunks_do_not_restart_the_message() {
        let emitter = test_emitter("s1");
        let mut receiver = emitter.subscribe_for_test();
        let state = Mutex::new(SessionUpdateState::default());
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hi")))
            .message_id(agent_client_protocol::v1::schema::MessageId::new("m1"));

        apply_message_chunk(&emitter, &state, &chunk);
        apply_message_chunk(&emitter, &state, &chunk);

        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        let starts = events
            .iter()
            .filter(|event| event.event_type == AgentEventType::MessageStarted)
            .count();
        assert_eq!(starts, 1, "a replayed chunk must not emit a second start");
    }
}

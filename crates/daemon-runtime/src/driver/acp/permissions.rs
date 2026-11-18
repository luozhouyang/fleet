//! ACP v2 permission and elicitation mapping.
//!
//! v1 mapping is shared with the legacy adapter (`mapper::v1`). The v2 request
//! shapes differ (the permission subject is structured rather than embedded in
//! a tool call), so v2 has its own mapping here.

use std::collections::{BTreeMap, BTreeSet};

use agent_client_protocol::v2::schema as v2s;
use fleet_core_common::{new_id, now};
use fleet_core_protocol::agent::{
    AgentPermissionOption, AgentPermissionRequest, AgentQuestion, AgentQuestionOption,
    AgentQuestionRequest, PermissionDecision, PermissionStatus, QuestionDecision, QuestionStatus,
};
use serde_json::Value;

use crate::error::RuntimeError;

/// Map a v2 permission request into the Fleet model.
pub fn map_permission_request(request: &v2s::RequestPermissionRequest) -> AgentPermissionRequest {
    let (tool_call_id, input) = match &request.subject {
        Some(v2s::RequestPermissionSubject::ToolCall(subject)) => (
            Some(subject.tool_call.tool_call_id.to_string()),
            serde_json::to_value(&subject.tool_call).unwrap_or(Value::Null),
        ),
        Some(subject) => (None, serde_json::to_value(subject).unwrap_or(Value::Null)),
        None => (None, Value::Null),
    };
    let tool_name = request
        .subject
        .as_ref()
        .and_then(|subject| match subject {
            v2s::RequestPermissionSubject::ToolCall(subject) => subject
                .tool_call
                .title
                .value()
                .cloned()
                .or_else(|| subject.tool_call.name.value().cloned()),
            v2s::RequestPermissionSubject::Command(subject) => Some(subject.command.clone()),
            _ => None,
        })
        .unwrap_or_else(|| request.title.clone());
    AgentPermissionRequest {
        id: new_id("permission"),
        session_id: request.session_id.to_string(),
        turn_id: None,
        tool_call_id,
        tool_name,
        description: request
            .description
            .clone()
            .unwrap_or_else(|| request.title.clone()),
        input,
        options: request
            .options
            .iter()
            .map(|option| AgentPermissionOption {
                option_id: option.option_id.to_string(),
                name: option.name.clone(),
                kind: permission_kind(&option.kind),
            })
            .collect(),
        status: PermissionStatus::Pending,
        created_at: now(),
        resolved_at: None,
        expires_at: None,
    }
}

/// Map a Fleet permission decision into a v2 response.
///
/// The original option id is preferred; otherwise the first `allow_*` or
/// `reject_*` option is selected. If no suitable option exists the request is
/// cancelled, matching the fail-closed policy.
pub fn map_permission_response(
    request: &AgentPermissionRequest,
    decision: &PermissionDecision,
) -> v2s::RequestPermissionResponse {
    let selected = decision.option_id.clone().or_else(|| {
        let prefix = if decision.approve { "allow" } else { "reject" };
        request
            .options
            .iter()
            .find(|option| option.kind.starts_with(prefix))
            .map(|option| option.option_id.clone())
    });
    match selected {
        Some(option_id) => v2s::RequestPermissionResponse::new(
            v2s::RequestPermissionOutcome::Selected(v2s::SelectedPermissionOutcome::new(option_id)),
        ),
        None => v2s::RequestPermissionResponse::new(v2s::RequestPermissionOutcome::Cancelled(
            v2s::CancelledPermissionOutcome::new(),
        )),
    }
}

fn permission_kind(kind: &v2s::PermissionOptionKind) -> String {
    match kind {
        v2s::PermissionOptionKind::AllowOnce => "allow_once",
        v2s::PermissionOptionKind::AllowAlways => "allow_always",
        v2s::PermissionOptionKind::RejectOnce => "reject_once",
        v2s::PermissionOptionKind::RejectAlways => "reject_always",
        _ => "unknown",
    }
    .to_string()
}

/// Map a v2 form elicitation into a Fleet question request.
///
/// URL elicitations and non-form modes are reported as unsupported so the
/// caller can decline them explicitly instead of pretending they were shown.
pub fn map_elicitation_request(
    request: &v2s::CreateElicitationRequest,
) -> Result<AgentQuestionRequest, RuntimeError> {
    let form = match &request.mode {
        v2s::ElicitationMode::Form(form) => form,
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
        v2s::ElicitationScope::Session(scope) => {
            scope.tool_call_id.as_ref().map(ToString::to_string)
        }
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

/// Map a Fleet question decision into a v2 elicitation response.
pub fn map_elicitation_response(
    request: &AgentQuestionRequest,
    decision: &QuestionDecision,
) -> v2s::CreateElicitationResponse {
    if decision.reject {
        return v2s::CreateElicitationResponse::new(v2s::ElicitationAction::Decline);
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
                v2s::ElicitationContentValue::StringArray(wire_values),
            );
        } else if let Some(selected) = wire_values.first() {
            content.insert(
                question.id.clone(),
                v2s::ElicitationContentValue::String(selected.clone()),
            );
        }
        if let Some(custom) = &answer.custom {
            content.insert(
                format!("{}_custom", question.id),
                v2s::ElicitationContentValue::String(custom.clone()),
            );
        }
    }
    v2s::CreateElicitationResponse::new(v2s::ElicitationAction::Accept(
        v2s::ElicitationAcceptAction::new().content(Some(content)),
    ))
}

fn map_question(
    id: &str,
    property: &v2s::ElicitationPropertySchema,
    allow_custom: bool,
) -> AgentQuestion {
    match property {
        v2s::ElicitationPropertySchema::String(schema) => {
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
                is_secret: matches!(
                    &schema.format,
                    Some(v2s::StringFormat::Other(format)) if format.eq_ignore_ascii_case("password")
                ),
            }
        }
        v2s::ElicitationPropertySchema::Array(schema) => {
            let options = match &schema.items {
                v2s::MultiSelectItems::String(items) => items
                    .values
                    .iter()
                    .map(|value| AgentQuestionOption {
                        label: value.clone(),
                        description: None,
                        value: None,
                    })
                    .collect(),
                v2s::MultiSelectItems::Titled(items) => items
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
        v2s::ElicitationPropertySchema::Number(schema) => free_form_question(
            id,
            schema.title.clone(),
            schema.description.clone(),
            allow_custom,
        ),
        v2s::ElicitationPropertySchema::Integer(schema) => free_form_question(
            id,
            schema.title.clone(),
            schema.description.clone(),
            allow_custom,
        ),
        v2s::ElicitationPropertySchema::Boolean(schema) => free_form_question(
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
    use fleet_core_protocol::agent::AgentQuestionAnswer;

    #[test]
    fn permission_falls_back_to_reject_option() {
        let request = AgentPermissionRequest {
            id: "p1".into(),
            session_id: "s1".into(),
            turn_id: None,
            tool_call_id: None,
            tool_name: "tool".into(),
            description: "d".into(),
            input: Value::Null,
            options: vec![
                AgentPermissionOption {
                    option_id: "allow".into(),
                    name: "Allow".into(),
                    kind: "allow_once".into(),
                },
                AgentPermissionOption {
                    option_id: "deny".into(),
                    name: "Deny".into(),
                    kind: "reject_once".into(),
                },
            ],
            status: PermissionStatus::Pending,
            created_at: now(),
            resolved_at: None,
            expires_at: None,
        };
        let response = map_permission_response(
            &request,
            &PermissionDecision {
                permission_id: "p1".into(),
                session_id: None,
                approve: false,
                option_id: None,
                message: None,
            },
        );
        match response.outcome {
            v2s::RequestPermissionOutcome::Selected(selected) => {
                assert_eq!(selected.option_id.to_string(), "deny");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn elicitation_answers_roundtrip() {
        let schema = v2s::ElicitationSchema::new().property(
            "question_0",
            v2s::StringPropertySchema::new()
                .title("Env".to_string())
                .one_of(vec![v2s::EnumOption::new("staging", "Staging")]),
            true,
        );
        let form = v2s::ElicitationFormMode::new(
            v2s::ElicitationScope::Session(v2s::ElicitationSessionScope::new(v2s::SessionId::new(
                "native",
            ))),
            schema,
        );
        let request = v2s::CreateElicitationRequest::new(
            v2s::ElicitationMode::Form(form),
            "Pick an environment",
        );
        let question = map_elicitation_request(&request).unwrap();
        assert_eq!(question.questions[0].id, "question_0");
        assert_eq!(question.questions[0].options[0].label, "Staging");
        let response = map_elicitation_response(
            &question,
            &QuestionDecision {
                question_id: question.id.clone(),
                session_id: None,
                answers: vec![AgentQuestionAnswer {
                    id: "question_0".into(),
                    selected: vec!["Staging".into()],
                    custom: None,
                }],
                reject: false,
                message: None,
            },
        );
        match response.action {
            v2s::ElicitationAction::Accept(accept) => {
                let content = accept.content.unwrap();
                assert!(matches!(
                    content.get("question_0"),
                    Some(v2s::ElicitationContentValue::String(value)) if value == "staging"
                ));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}

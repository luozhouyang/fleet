//! Pure ACP mapper tests: permissions, elicitations and form questions.

use agent_client_protocol::v1::schema::{
    CreateElicitationRequest, ElicitationAction, ElicitationFormMode, ElicitationSchema,
    ElicitationSessionScope, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use fleet_core_protocol::agent::{AgentQuestionAnswer, PermissionDecision, QuestionDecision};
use fleet_daemon_runtime::acp::mapper::{
    map_elicitation_request, map_elicitation_response, map_permission_request,
    map_permission_response,
};

#[test]
fn ask_user_question_elicitation_maps_to_two_questions() {
    let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "question_0": {
                "type": "string",
                "title": "Deploy",
                "description": "Which environment?",
                "oneOf": [
                    {"const": "staging", "title": "staging", "description": "Safe"},
                    {"const": "production", "title": "production", "description": "Live"}
                ]
            },
            "question_0_custom": {"type": "string", "title": "Other"},
            "question_1": {
                "type": "array",
                "title": "Extras",
                "description": "Pick extras",
                "items": {"anyOf": [{"const": "logs", "title": "logs"}]}
            },
            "question_1_custom": {"type": "string", "title": "Other"}
        }
    }))
    .unwrap();
    let request = CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("session_1"), schema),
        "Please answer the following questions.",
    );
    let mapped = map_elicitation_request(&request).expect("mapped question");
    assert_eq!(mapped.questions.len(), 2);
    assert_eq!(mapped.questions[0].id, "question_0");
    assert_eq!(mapped.questions[0].options[0].label, "staging");
    assert!(mapped.questions[0].allow_custom);
    assert!(mapped.questions[1].multi_select);
    assert_eq!(mapped.questions[1].options[0].label, "logs");
}

#[test]
fn permission_request_and_response_round_trip() {
    let request = RequestPermissionRequest::new(
        "session_1",
        ToolCallUpdate::new(
            "tool_1",
            ToolCallUpdateFields::new()
                .title("Run tests")
                .status(ToolCallStatus::Pending),
        ),
        vec![
            PermissionOption::new("allow_once", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("reject_once", "Reject", PermissionOptionKind::RejectOnce),
        ],
    );

    let mapped = map_permission_request(&request);
    assert_eq!(mapped.tool_call_id.as_deref(), Some("tool_1"));
    assert_eq!(mapped.tool_name, "Run tests");
    assert_eq!(mapped.options.len(), 2);
    assert_eq!(mapped.options[0].kind, "allow_once");

    let explicit = map_permission_response(
        &mapped,
        &PermissionDecision {
            permission_id: mapped.id.clone(),
            session_id: None,
            approve: true,
            option_id: Some("allow_once".to_string()),
            message: None,
        },
    );
    match explicit.outcome {
        RequestPermissionOutcome::Selected(selected) => {
            assert_eq!(selected.option_id.to_string(), "allow_once");
        }
        _ => panic!("expected an explicit selected outcome"),
    }

    // With no explicit option, approve maps to the first allow_* option.
    let fallback = map_permission_response(
        &mapped,
        &PermissionDecision {
            permission_id: mapped.id.clone(),
            session_id: None,
            approve: true,
            option_id: None,
            message: None,
        },
    );
    match fallback.outcome {
        RequestPermissionOutcome::Selected(selected) => {
            assert_eq!(selected.option_id.to_string(), "allow_once");
        }
        _ => panic!("expected the allow fallback"),
    }

    // Reject maps to reject_*.
    let rejected = map_permission_response(
        &mapped,
        &PermissionDecision {
            permission_id: mapped.id.clone(),
            session_id: None,
            approve: false,
            option_id: None,
            message: None,
        },
    );
    match rejected.outcome {
        RequestPermissionOutcome::Selected(selected) => {
            assert_eq!(selected.option_id.to_string(), "reject_once");
        }
        _ => panic!("expected the reject fallback"),
    }
}

#[test]
fn elicitation_response_includes_selected_and_custom_values() {
    let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "question_0": {
                "type": "string",
                "title": "Deploy",
                "oneOf": [
                    {"const": "staging", "title": "staging"}
                ]
            },
            "question_0_custom": {"type": "string", "title": "Other"},
            "question_1": {
                "type": "array",
                "title": "Extras",
                "items": {"type": "string", "enum": ["logs", "metrics"]}
            }
        }
    }))
    .unwrap();
    let request = CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("session_1"), schema),
        "Please answer.",
    );
    let mapped = map_elicitation_request(&request).expect("mapped question");

    let response = map_elicitation_response(
        &mapped,
        &QuestionDecision {
            question_id: mapped.id.clone(),
            session_id: None,
            answers: vec![
                AgentQuestionAnswer {
                    id: "question_0".to_string(),
                    selected: vec!["staging".to_string()],
                    custom: Some("canary".to_string()),
                },
                AgentQuestionAnswer {
                    id: "question_1".to_string(),
                    selected: vec!["logs".to_string(), "metrics".to_string()],
                    custom: None,
                },
            ],
            reject: false,
            message: None,
        },
    );

    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["action"], "accept");
    assert_eq!(value["content"]["question_0"], "staging");
    assert_eq!(value["content"]["question_0_custom"], "canary");
    assert_eq!(
        value["content"]["question_1"],
        serde_json::json!(["logs", "metrics"])
    );
}

#[test]
fn rejected_elicitation_is_declined() {
    let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "question_0": {"type": "string", "title": "Deploy"}
        }
    }))
    .unwrap();
    let request = CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("session_1"), schema),
        "Please answer.",
    );
    let mapped = map_elicitation_request(&request).expect("mapped question");
    let response = map_elicitation_response(
        &mapped,
        &QuestionDecision {
            question_id: mapped.id.clone(),
            session_id: None,
            answers: Vec::new(),
            reject: true,
            message: None,
        },
    );
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["action"], "decline");
    assert!(matches!(response.action, ElicitationAction::Decline));
}

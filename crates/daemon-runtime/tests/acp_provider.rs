//! ACP provider integration tests against a deterministic fake ACP agent.
//!
//! The fixture is a Node.js script that implements just enough of ACP v1 to
//! exercise the handshake, session updates, permission requests and form
//! elicitations without credentials or a real model.

use std::sync::Arc;
use std::time::Duration;

use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentPermissionRequest, AgentQuestionAnswer, AgentQuestionRequest,
    AgentStartConfig, PermissionDecision, QuestionDecision, SendMessageRequest,
};
use fleet_daemon_runtime::{AcpProvider, AgentRuntime, ProviderRegistry};
use tokio::sync::broadcast;
use tokio::time::Instant;

fn fixture_path() -> String {
    format!(
        "{}/tests/fixtures/fake-acp-agent.mjs",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn runtime_with_acp(args: Vec<String>) -> AgentRuntime {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(AcpProvider::new("node", args)));
    AgentRuntime::new(registry, 4096)
}

fn start_config(cwd: &std::path::Path) -> AgentStartConfig {
    AgentStartConfig {
        cwd: cwd.to_string_lossy().to_string(),
        title: Some("acp test".to_string()),
        ..AgentStartConfig::default()
    }
}

async fn next_matching(
    receiver: &mut broadcast::Receiver<AgentEvent>,
    predicate: impl Fn(&AgentEvent) -> bool,
) -> AgentEvent {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout_at(deadline, receiver.recv())
            .await
            .expect("timed out waiting for ACP event")
            .expect("agent event bus closed");
        if predicate(&event) {
            return event;
        }
    }
}

#[tokio::test]
async fn acp_provider_maps_permission_and_completes_turn() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_with_acp(vec![fixture_path()]);
    let mut events = runtime.subscribe();

    let session = runtime
        .start_session("machine_test", "claude-acp", start_config(temp.path()))
        .await
        .expect("start ACP session");

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "hello".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send ACP message");

    let delta = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta
    })
    .await;
    assert_eq!(delta.payload["text"], "Hello from ACP");
    assert_eq!(delta.message_id.as_deref(), Some("msg_1"));

    let permission_event = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    let permission: AgentPermissionRequest =
        serde_json::from_value(permission_event.payload["permission"].clone())
            .expect("permission payload");
    assert_eq!(permission.tool_name, "Run tests");
    assert_eq!(permission.session_id, session.id);
    assert_eq!(permission.options[0].option_id, "allow_once");

    runtime
        .respond_permission(PermissionDecision {
            permission_id: permission.id.clone(),
            session_id: Some(session.id.clone()),
            approve: true,
            option_id: Some("allow_once".to_string()),
            message: None,
        })
        .await
        .expect("respond to ACP permission");

    let completed = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    assert_eq!(completed.payload["status"], "completed");

    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_provider_maps_form_question_and_answers() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_with_acp(vec![fixture_path(), "--question".to_string()]);
    let mut events = runtime.subscribe();

    let session = runtime
        .start_session("machine_test", "claude-acp", start_config(temp.path()))
        .await
        .expect("start ACP session");

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "ask me".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send ACP message");

    let question_event = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::QuestionRequested
    })
    .await;
    let question: AgentQuestionRequest =
        serde_json::from_value(question_event.payload["question"].clone())
            .expect("question payload");
    assert_eq!(question.session_id, session.id);
    assert_eq!(question.questions.len(), 1);
    assert_eq!(question.questions[0].id, "question_0");
    assert_eq!(question.questions[0].options[0].label, "staging");
    assert!(question.questions[0].allow_custom);

    runtime
        .respond_question(QuestionDecision {
            question_id: question.id.clone(),
            session_id: Some(session.id.clone()),
            answers: vec![AgentQuestionAnswer {
                id: "question_0".to_string(),
                selected: vec!["staging".to_string()],
                custom: Some("canary".to_string()),
            }],
            reject: false,
            message: None,
        })
        .await
        .expect("respond to ACP question");

    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::QuestionAnswered
    })
    .await;
    let completed = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    assert_eq!(completed.payload["status"], "completed");

    runtime.shutdown_session(&session.id).await.unwrap();
}

//! Runtime integration tests using the in-process mock provider.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use agent_fleet_agent_runtime::{
    AgentRuntime, GenericProvider, MockProvider, ProviderRegistry, RuntimeError,
};
use agent_fleet_protocol::agent::{
    AgentEvent, AgentEventType, AgentSession, AgentStartConfig, Attachment,
    MessageCompletedPayload, MessageDeltaPayload, PermissionDecision, PermissionEventPayload,
    PermissionStatus, SendMessageRequest, SessionErrorPayload, SessionStatus, ToolEventPayload,
    ToolStatusPayload, TurnStatus,
};
use tokio::sync::broadcast;

fn runtime_with_mock() -> AgentRuntime {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    AgentRuntime::new(registry, 4096)
}

fn start_config(delay_ms: u64) -> AgentStartConfig {
    AgentStartConfig {
        cwd: std::env::temp_dir().to_string_lossy().to_string(),
        title: Some("mock test".to_string()),
        extra: serde_json::json!({"delay_ms": delay_ms}),
        ..AgentStartConfig::default()
    }
}

async fn start_mock(
    runtime: &AgentRuntime,
    delay_ms: u64,
) -> (AgentSession, broadcast::Receiver<AgentEvent>) {
    let receiver = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "mock", start_config(delay_ms))
        .await
        .expect("mock session should start");
    (session, receiver)
}

async fn next_matching(
    receiver: &mut broadcast::Receiver<AgentEvent>,
    predicate: impl Fn(&AgentEvent) -> bool,
) -> AgentEvent {
    let wait = async {
        loop {
            match receiver.recv().await {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => panic!("event bus closed"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait)
        .await
        .expect("timed out waiting for event")
}

async fn collect_until(
    receiver: &mut broadcast::Receiver<AgentEvent>,
    stop: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let collect = async {
        let mut events = Vec::new();
        loop {
            let event = match receiver.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => panic!("event bus closed"),
            };
            let done = stop(&event);
            events.push(event);
            if done {
                return events;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), collect)
        .await
        .expect("timed out collecting events")
}

async fn await_status(runtime: &AgentRuntime, session_id: &str, expected: SessionStatus) {
    for _ in 0..300 {
        if runtime
            .get_session(session_id)
            .await
            .map(|session| session.status)
            == Some(expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "session {session_id} did not reach {expected}; current: {:?}",
        runtime
            .get_session(session_id)
            .await
            .map(|session| session.status)
    );
}

fn send(content: &str) -> SendMessageRequest {
    SendMessageRequest {
        content: content.to_string(),
        attachments: Vec::new(),
    }
}

#[tokio::test]
async fn mock_happy_path_emits_ordered_events() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .send_message(&session.id, send("hello world"))
        .await
        .expect("send_message");

    let events = collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;

    let types: Vec<AgentEventType> = events
        .iter()
        .map(|event| event.event_type)
        .filter(|event_type| {
            matches!(
                event_type,
                AgentEventType::SessionCreated
                    | AgentEventType::SessionStarted
                    | AgentEventType::TurnStarted
                    | AgentEventType::MessageStarted
                    | AgentEventType::MessageDelta
                    | AgentEventType::MessageCompleted
                    | AgentEventType::TurnCompleted
            )
        })
        .collect();
    assert_eq!(
        &types[..4],
        &[
            AgentEventType::SessionCreated,
            AgentEventType::SessionStarted,
            AgentEventType::TurnStarted,
            AgentEventType::MessageStarted,
        ]
    );
    assert_eq!(
        &types[types.len() - 2..],
        &[
            AgentEventType::MessageCompleted,
            AgentEventType::TurnCompleted,
        ]
    );

    let streamed: String = events
        .iter()
        .filter(|event| event.event_type == AgentEventType::MessageDelta)
        .filter_map(|event| {
            serde_json::from_value::<MessageDeltaPayload>(event.payload.clone()).ok()
        })
        .map(|payload| payload.text)
        .collect();
    assert_eq!(streamed, "Echo: hello world");
    assert!(events
        .iter()
        .all(|event| event.session_id == session.id && event.sequence > 0));

    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    let detail = runtime
        .session_detail(&session.id)
        .await
        .expect("session detail");
    assert_eq!(detail.turns.len(), 1);
    assert_eq!(detail.turns[0].status, TurnStatus::Completed);
    assert!(detail
        .messages
        .iter()
        .any(|message| message.content == "Echo: hello world"));
    assert!(detail
        .messages
        .iter()
        .any(|message| message.content == "hello world"));
}

#[tokio::test]
async fn tool_permission_approval_completes_tool() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .send_message(&session.id, send("/tool"))
        .await
        .expect("send_message");

    let requested = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    let payload: PermissionEventPayload =
        serde_json::from_value(requested.payload.clone()).expect("permission payload");

    runtime
        .respond_permission(PermissionDecision {
            permission_id: payload.permission.id.clone(),
            approve: true,
            message: None,
        })
        .await
        .expect("approve permission");

    let completed = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::ToolCompleted
    })
    .await;
    let tool: ToolEventPayload =
        serde_json::from_value(completed.payload.clone()).expect("tool payload");
    assert_eq!(tool.tool.output["stdout"], "hello\n");

    let message = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::MessageCompleted
    })
    .await;
    let message: MessageCompletedPayload =
        serde_json::from_value(message.payload.clone()).expect("message payload");
    assert!(message.message.content.contains("Tool finished."));

    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    let detail = runtime
        .session_detail(&session.id)
        .await
        .expect("session detail");
    let permission = detail
        .permissions
        .iter()
        .find(|permission| permission.id == payload.permission.id)
        .expect("recorded permission");
    assert_eq!(permission.status, PermissionStatus::Approved);
}

#[tokio::test]
async fn tool_permission_rejection_fails_tool() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .send_message(&session.id, send("/tool"))
        .await
        .expect("send_message");
    let requested = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    let payload: PermissionEventPayload =
        serde_json::from_value(requested.payload.clone()).expect("permission payload");

    runtime
        .respond_permission(PermissionDecision {
            permission_id: payload.permission.id.clone(),
            approve: false,
            message: None,
        })
        .await
        .expect("reject permission");

    let failed = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::ToolFailed
    })
    .await;
    let payload: ToolStatusPayload =
        serde_json::from_value(failed.payload.clone()).expect("tool failure payload");
    assert_eq!(
        payload.status,
        agent_fleet_protocol::agent::ToolCallStatus::Failed
    );
    assert_eq!(payload.error.as_deref(), Some("rejected by user"));

    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;
}

#[tokio::test]
async fn fail_scenario_reports_session_error() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .send_message(&session.id, send("/fail"))
        .await
        .expect("send_message");

    let failed = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnFailed
    })
    .await;
    assert_eq!(failed.payload["error"]["code"], "mock_failure");

    let error = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::SessionError
    })
    .await;
    let payload: SessionErrorPayload =
        serde_json::from_value(error.payload.clone()).expect("error payload");
    assert_eq!(payload.error.code, "mock_failure");

    await_status(&runtime, &session.id, SessionStatus::Failed).await;
    let session = runtime.get_session(&session.id).await.expect("session");
    assert_eq!(session.last_error.expect("last error").code, "mock_failure");
}

#[tokio::test]
async fn interrupt_cuts_slow_stream_short() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .send_message(&session.id, send("/slow"))
        .await
        .expect("send_message");
    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::MessageDelta
    })
    .await;

    runtime.interrupt(&session.id).await.expect("interrupt");
    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnInterrupted
    })
    .await;

    let completed = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::MessageCompleted
    })
    .await;
    let payload: MessageCompletedPayload =
        serde_json::from_value(completed.payload.clone()).expect("message payload");
    assert!(
        !payload.message.content.is_empty() && payload.message.content.starts_with("word"),
        "unexpected partial content: {:?}",
        payload.message.content
    );

    await_status(&runtime, &session.id, SessionStatus::Interrupted).await;
}

#[tokio::test]
async fn resume_reactivates_interrupted_session() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .send_message(&session.id, send("/slow"))
        .await
        .expect("send_message");
    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::MessageDelta
    })
    .await;
    runtime.interrupt(&session.id).await.expect("interrupt");
    await_status(&runtime, &session.id, SessionStatus::Interrupted).await;

    runtime.resume(&session.id).await.expect("resume");
    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::SessionResumed
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::Running).await;
}

#[tokio::test]
async fn shutdown_terminates_and_rejects_new_messages() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .shutdown_session(&session.id)
        .await
        .expect("shutdown");
    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::SessionTerminated
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::Terminated).await;

    let error = runtime
        .send_message(&session.id, send("too late"))
        .await
        .expect_err("terminated session must reject messages");
    assert!(matches!(error, RuntimeError::InvalidState(_)));
    assert_eq!(
        runtime
            .get_session(&session.id)
            .await
            .expect("session")
            .status,
        SessionStatus::Terminated
    );
}

#[tokio::test]
async fn concurrent_sessions_are_isolated_and_sequences_are_monotonic() {
    let runtime = runtime_with_mock();
    let mut receiver = runtime.subscribe();
    let first = runtime
        .start_session("machine_test", "mock", start_config(1))
        .await
        .expect("first session");
    let second = runtime
        .start_session("machine_test", "mock", start_config(1))
        .await
        .expect("second session");

    runtime
        .send_message(&first.id, send("alpha"))
        .await
        .expect("first message");
    runtime
        .send_message(&second.id, send("beta"))
        .await
        .expect("second message");

    let wanted: HashSet<String> = [first.id.clone(), second.id.clone()].into_iter().collect();
    let events = collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted && wanted.contains(&event.session_id)
    })
    .await;

    // Drain until both sessions have a turn.completed and status is idle.
    let mut all = events;
    loop {
        let completed: HashSet<String> = all
            .iter()
            .filter(|event| event.event_type == AgentEventType::TurnCompleted)
            .map(|event| event.session_id.clone())
            .collect();
        if completed.len() == 2 {
            break;
        }
        let extra = collect_until(&mut receiver, |event| {
            event.event_type == AgentEventType::TurnCompleted
        })
        .await;
        all.extend(extra);
    }

    let mut deltas: BTreeMap<String, String> = BTreeMap::new();
    let mut sequences: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for event in &all {
        sequences
            .entry(event.session_id.clone())
            .or_default()
            .push(event.sequence);
        if event.event_type == AgentEventType::MessageDelta {
            if let Ok(payload) =
                serde_json::from_value::<MessageDeltaPayload>(event.payload.clone())
            {
                deltas
                    .entry(event.session_id.clone())
                    .or_default()
                    .push_str(&payload.text);
            }
        }
    }
    assert_eq!(
        deltas.get(&first.id).map(String::as_str),
        Some("Echo: alpha")
    );
    assert_eq!(
        deltas.get(&second.id).map(String::as_str),
        Some("Echo: beta")
    );
    for (session_id, sequence) in &sequences {
        assert_eq!(sequence[0], 1, "session {session_id} must start at 1");
        assert!(
            sequence.windows(2).all(|pair| pair[0] < pair[1]),
            "session {session_id} sequences must be strictly increasing: {sequence:?}"
        );
    }
}

#[tokio::test]
async fn sessions_are_persisted_when_storage_is_attached() {
    let database = agent_fleet_storage::Database::connect_in_memory()
        .await
        .expect("in-memory database");
    database.migrate().await.expect("migrations");

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));

    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("persist me"))
        .await
        .expect("send_message");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    let mut persisted = None;
    for _ in 0..100 {
        persisted = database
            .get_session(&session.id)
            .await
            .expect("database query");
        if persisted
            .as_ref()
            .is_some_and(|stored| stored.status == SessionStatus::WaitingForInput)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        persisted.is_some_and(|stored| stored.status == SessionStatus::WaitingForInput),
        "session was not persisted with its final status"
    );
}

#[tokio::test]
async fn sequential_turns_use_fresh_turn_ids() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    runtime
        .send_message(&session.id, send("one"))
        .await
        .expect("first message");
    let first = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    runtime
        .send_message(&session.id, send("two"))
        .await
        .expect("second message");
    let second = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;

    let first_turn = first.turn_id.clone().expect("first turn id");
    let second_turn = second.turn_id.clone().expect("second turn id");
    assert_ne!(first_turn, second_turn);

    let detail = runtime
        .session_detail(&session.id)
        .await
        .expect("session detail");
    assert_eq!(detail.turns.len(), 2);
    assert!(detail
        .turns
        .iter()
        .all(|turn| turn.status == TurnStatus::Completed));
}

#[tokio::test]
async fn session_detail_and_get_session_track_state() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;

    let created = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::SessionCreated
    })
    .await;
    assert_eq!(created.session_id, session.id);

    runtime
        .send_message(&session.id, send("detail"))
        .await
        .expect("send_message");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    let stored = runtime.get_session(&session.id).await.expect("session");
    assert_eq!(stored.provider, "mock");
    assert_eq!(stored.machine_id, "machine_test");
    let detail = runtime.session_detail(&session.id).await.expect("detail");
    assert_eq!(detail.session.id, session.id);
    assert_eq!(detail.turns.len(), 1);
    assert!(detail.messages.len() >= 2);
}

#[tokio::test]
async fn missing_sessions_and_permissions_return_errors() {
    let runtime = runtime_with_mock();

    assert!(runtime.get_session("missing").await.is_none());
    assert!(runtime.session_detail("missing").await.is_none());
    assert!(matches!(
        runtime.send_message("missing", send("hi")).await,
        Err(RuntimeError::SessionNotFound(_))
    ));
    assert!(matches!(
        runtime.interrupt("missing").await,
        Err(RuntimeError::SessionNotFound(_))
    ));
    assert!(matches!(
        runtime.resume("missing").await,
        Err(RuntimeError::SessionNotFound(_))
    ));
    assert!(matches!(
        runtime.shutdown_session("missing").await,
        Err(RuntimeError::SessionNotFound(_))
    ));
    assert!(matches!(
        runtime
            .respond_permission(PermissionDecision {
                permission_id: "missing".to_string(),
                approve: true,
                message: None,
            })
            .await,
        Err(RuntimeError::PermissionNotFound(_))
    ));
}

#[tokio::test]
async fn illegal_resume_after_shutdown_is_rejected() {
    let runtime = runtime_with_mock();
    let (session, _receiver) = start_mock(&runtime, 1).await;
    runtime
        .shutdown_session(&session.id)
        .await
        .expect("shutdown");
    assert!(matches!(
        runtime.resume(&session.id).await,
        Err(RuntimeError::InvalidState(_))
    ));
}

#[tokio::test]
async fn unsupported_attachments_are_rejected() {
    let runtime = runtime_with_mock();
    let (session, _receiver) = start_mock(&runtime, 1).await;
    let error = runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "look".to_string(),
                attachments: vec![Attachment {
                    name: "image.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    data_base64: "aGVsbG8=".to_string(),
                }],
            },
        )
        .await
        .expect_err("mock does not accept images");
    assert!(matches!(error, RuntimeError::Unsupported(_)));
}

#[test]
fn provider_registry_prefers_mock_and_lists_infos() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(GenericProvider::new("cat", Vec::<String>::new())));
    registry.register(Arc::new(MockProvider::new()));

    let default = registry.default_provider().expect("default provider");
    assert_eq!(default.provider_id(), "mock");
    let infos = registry.infos();
    assert_eq!(infos.len(), 2);
    assert!(infos.iter().any(|info| info.id == "mock"));
    assert!(infos.iter().any(|info| info.id == "generic"));
    assert!(registry.get("missing").is_none());
}

#[test]
fn provider_registry_falls_back_to_first_provider() {
    let empty = ProviderRegistry::new();
    assert!(empty.default_provider().is_none());

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(GenericProvider::new("cat", Vec::<String>::new())));
    assert_eq!(
        registry
            .default_provider()
            .expect("default provider")
            .provider_id(),
        "generic"
    );
}

#[tokio::test]
async fn rejects_a_second_message_while_a_turn_is_in_flight() {
    let runtime = runtime_with_mock();
    let (session, _receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("/never"))
        .await
        .expect("first message");

    let error = runtime
        .send_message(&session.id, send("second"))
        .await
        .expect_err("overlapping send must fail");
    assert!(matches!(error, RuntimeError::InvalidState(_)));

    runtime.interrupt(&session.id).await.expect("interrupt");
    await_status(&runtime, &session.id, SessionStatus::Interrupted).await;
}

#[tokio::test]
async fn resume_requires_an_interrupted_session() {
    let runtime = runtime_with_mock();
    let (session, _receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("/never"))
        .await
        .expect("message");

    let error = runtime
        .resume(&session.id)
        .await
        .expect_err("resume while running must fail");
    assert!(matches!(error, RuntimeError::InvalidState(_)));

    runtime.interrupt(&session.id).await.expect("interrupt");
    await_status(&runtime, &session.id, SessionStatus::Interrupted).await;
    runtime
        .resume(&session.id)
        .await
        .expect("resume interrupted");
}

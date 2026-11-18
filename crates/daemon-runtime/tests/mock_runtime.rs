//! Runtime integration tests using the in-process mock provider.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentQuestion, AgentQuestionAnswer, AgentQuestionOption,
    AgentQuestionRequest, AgentSession, AgentStartConfig, Attachment, MessageCompletedPayload,
    MessageDeltaPayload, PermissionDecision, PermissionEventPayload, PermissionStatus,
    QuestionDecision, QuestionEventPayload, QuestionStatus, SendMessageRequest,
    SessionErrorPayload, SessionStatus, ToolCallStatus, ToolEventPayload, ToolStatusPayload,
    TurnStatus,
};
use fleet_daemon_runtime::{
    AgentRuntime, ClaudeProvider, GenericProvider, MockProvider, ProviderRegistry, RuntimeError,
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
        &types[..5],
        &[
            AgentEventType::SessionCreated,
            AgentEventType::SessionStarted,
            AgentEventType::TurnStarted,
            AgentEventType::MessageCompleted,
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

    let user_completed = events
        .iter()
        .filter(|event| event.event_type == AgentEventType::MessageCompleted)
        .find_map(|event| {
            serde_json::from_value::<MessageCompletedPayload>(event.payload.clone()).ok()
        })
        .expect("user message completion");
    assert_eq!(
        user_completed.message.role,
        fleet_core_protocol::agent::MessageRole::User
    );
    assert_eq!(user_completed.message.content, "hello world");

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
            session_id: None,
            option_id: None,
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
    let completed_tool_id = tool.tool.id.clone();

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
    let tool = detail
        .tool_calls
        .iter()
        .find(|record| record.id == completed_tool_id)
        .expect("recorded tool call");
    assert_eq!(tool.status, ToolCallStatus::Completed);
    assert_eq!(tool.output["stdout"], "hello\n");
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
            session_id: None,
            option_id: None,
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
        fleet_core_protocol::agent::ToolCallStatus::Failed
    );
    assert_eq!(payload.error.as_deref(), Some("rejected by user"));

    next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    let detail = runtime
        .session_detail(&session.id)
        .await
        .expect("session detail");
    let failed_tool = detail
        .tool_calls
        .iter()
        .find(|tool| tool.name == "Bash")
        .expect("recorded failed tool call");
    assert_eq!(failed_tool.status, ToolCallStatus::Failed);
    assert_eq!(failed_tool.error.as_deref(), Some("rejected by user"));
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
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
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
            .sessions
            .get(&session.id)
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
async fn failed_provider_start_removes_the_ghost_session() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(ClaudeProvider::new(
        "definitely-not-a-claude-binary",
    )));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));
    let mut receiver = runtime.subscribe();

    let error = runtime
        .start_session("machine_test", "claude", start_config(1))
        .await
        .expect_err("missing provider binary must fail");
    assert!(matches!(error, RuntimeError::ProviderUnavailable(_)));
    assert!(runtime.list_sessions().await.is_empty());
    assert!(runtime.get_session("missing").await.is_none());

    let events = collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::SessionDeleted
    })
    .await;
    assert!(events
        .iter()
        .any(|event| event.event_type == AgentEventType::SessionCreated));
    assert!(database.sessions.list(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn turns_and_messages_are_persisted() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));

    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("persist history"))
        .await
        .expect("send_message");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;
    for _ in 0..100 {
        if database.messages.list(&session.id).await.unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let turns = database.turns.list(&session.id).await.expect("turns");
    let messages = database.messages.list(&session.id).await.expect("messages");
    assert_eq!(turns.len(), 1);
    assert!(messages
        .iter()
        .any(|message| message.content == "persist history"));
    assert!(messages
        .iter()
        .any(|message| message.content == "Echo: persist history"));
}

#[tokio::test]
async fn processed_events_carry_outbox_cursors_only_for_durable_events() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));
    let mut processed = runtime.subscribe_processed();

    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("cursor me"))
        .await
        .expect("send_message");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;

    let mut events = Vec::new();
    for _ in 0..200 {
        match tokio::time::timeout(Duration::from_millis(100), processed.recv()).await {
            Ok(Ok(processed)) => {
                let done = processed.event.event_type == AgentEventType::TurnCompleted;
                events.push(processed);
                if done {
                    break;
                }
            }
            _ => break,
        }
    }

    let created = events
        .iter()
        .find(|processed| processed.event.event_type == AgentEventType::SessionCreated)
        .expect("session created");
    assert!(created.cursor.is_some());
    let delta = events
        .iter()
        .find(|processed| processed.event.event_type == AgentEventType::MessageDelta)
        .expect("streaming delta");
    assert!(delta.cursor.is_none(), "deltas are not durable");
    let completed = events
        .iter()
        .find(|processed| processed.event.event_type == AgentEventType::MessageCompleted)
        .expect("message completed");
    assert!(completed.cursor.is_some());

    let mut cursors = events
        .iter()
        .filter_map(|processed| processed.cursor)
        .collect::<Vec<_>>();
    let before = cursors.len();
    cursors.dedup();
    assert_eq!(cursors.len(), before, "cursors must increase");
    assert!(cursors.windows(2).all(|pair| pair[0] < pair[1]));
}

#[tokio::test]
async fn durable_events_are_enqueued_in_the_outbox() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));

    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("outbox me"))
        .await
        .expect("send_message");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    let mut pending = Vec::new();
    for _ in 0..100 {
        pending = database.outbox.pending(1_000).await.expect("outbox");
        let types = pending
            .iter()
            .map(|entry| entry.event.event_type)
            .collect::<Vec<_>>();
        if types.contains(&AgentEventType::TurnCompleted)
            && types.contains(&AgentEventType::MessageCompleted)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let types = pending
        .iter()
        .map(|entry| entry.event.event_type)
        .collect::<Vec<_>>();
    assert!(types.contains(&AgentEventType::SessionCreated));
    assert!(types.contains(&AgentEventType::TurnCompleted));
    assert!(types.contains(&AgentEventType::MessageCompleted));
    assert!(
        !types.contains(&AgentEventType::MessageDelta),
        "ephemeral deltas must not enter the outbox: {types:?}"
    );
    assert!(
        pending
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence),
        "outbox sequences must be increasing"
    );

    let last = pending.last().expect("outbox entries").sequence;
    let mut cursor = 0;
    for entry in &pending {
        cursor = database.outbox.ack(entry.sequence).await.expect("ack");
    }
    assert_eq!(cursor, last);
    assert!(database.outbox.pending(1_000).await.unwrap().is_empty());
    assert_eq!(database.outbox.cursor().await.unwrap(), last);
}

#[tokio::test]
async fn restored_active_sessions_are_read_only_and_keep_history() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));

    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("survive restart"))
        .await
        .expect("send_message");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;
    for _ in 0..100 {
        if database.messages.list(&session.id).await.unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Simulate a session persisted by the pre-driver runtime: only the
    // legacy `provider` field exists, with no agent/driver metadata.
    let mut stored = database
        .sessions
        .get(&session.id)
        .await
        .unwrap()
        .expect("stored session");
    stored.metadata = serde_json::json!({"provider_session_id": "legacy_native"});
    database.sessions.upsert(&stored).await.unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let restored = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));
    restored
        .restore_sessions("machine_test")
        .await
        .expect("restore");

    let restored_session = restored
        .get_session(&session.id)
        .await
        .expect("restored session");
    assert_eq!(restored_session.status, SessionStatus::Interrupted);
    assert_eq!(
        restored_session
            .metadata
            .get("restored")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    // Legacy sessions are migrated in memory: the provider id keeps working
    // as an agent definition id and the driver is recorded for diagnostics.
    assert_eq!(
        restored_session.metadata["agent"]["definition_id"],
        session.provider
    );
    assert!(restored_session.metadata["agent"]["driver_id"].is_string());
    let detail = restored.session_detail(&session.id).await.expect("detail");
    assert!(detail
        .messages
        .iter()
        .any(|message| message.content == "survive restart"));
    assert!(detail
        .messages
        .iter()
        .any(|message| message.content == "Echo: survive restart"));
    let error = restored
        .send_message(&session.id, send("should fail"))
        .await
        .expect_err("restored sessions must be read-only");
    assert!(matches!(error, RuntimeError::Unsupported(_)));
}

#[tokio::test]
async fn tool_calls_are_persisted_and_restored() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));

    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("/tool"))
        .await
        .expect("send_message");
    let requested = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    let permission: PermissionEventPayload =
        serde_json::from_value(requested.payload.clone()).expect("permission payload");
    runtime
        .respond_permission(PermissionDecision {
            permission_id: permission.permission.id,
            session_id: None,
            option_id: None,
            approve: true,
            message: None,
        })
        .await
        .expect("approve permission");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    for _ in 0..100 {
        if !database
            .tool_calls
            .list(&session.id)
            .await
            .unwrap()
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let stored = database.tool_calls.list(&session.id).await.expect("tools");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].status, ToolCallStatus::Completed);
    assert_eq!(stored[0].output["stdout"], "hello\n");

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let restored = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));
    restored
        .restore_sessions("machine_test")
        .await
        .expect("restore");
    let detail = restored
        .session_detail(&session.id)
        .await
        .expect("restored detail");
    assert_eq!(detail.tool_calls.len(), 1);
    assert_eq!(detail.tool_calls[0].id, stored[0].id);
    assert_eq!(detail.tool_calls[0].status, ToolCallStatus::Completed);
}

#[tokio::test]
async fn restore_closes_running_turns_tools_and_permissions() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut session = AgentSession::new(
        "machine_test",
        "mock",
        "mock-agent",
        std::env::temp_dir().to_string_lossy().to_string(),
    );
    session.id = "s_restore".to_string();
    session.status = SessionStatus::Running;
    database.sessions.upsert(&session).await.unwrap();

    let now = fleet_core_common::now();
    database
        .turns
        .upsert(&fleet_core_protocol::agent::AgentTurn {
            id: "turn_restore".to_string(),
            session_id: session.id.clone(),
            status: TurnStatus::Running,
            started_at: now,
            completed_at: None,
            error: None,
        })
        .await
        .unwrap();
    database
        .tool_calls
        .upsert(&fleet_core_protocol::agent::AgentToolCall {
            id: "call_restore".to_string(),
            session_id: session.id.clone(),
            turn_id: Some("turn_restore".to_string()),
            name: "Bash".to_string(),
            status: ToolCallStatus::Running,
            input: serde_json::json!({"command": "sleep 100"}),
            output: serde_json::Value::Null,
            error: None,
            started_at: now,
            completed_at: None,
        })
        .await
        .unwrap();
    database
        .permissions
        .upsert(&fleet_core_protocol::agent::AgentPermissionRequest {
            id: "perm_restore".to_string(),
            session_id: session.id.clone(),
            turn_id: Some("turn_restore".to_string()),
            tool_call_id: Some("call_restore".to_string()),
            tool_name: "Bash".to_string(),
            description: "Run a command".to_string(),
            input: serde_json::json!({"command": "sleep 100"}),
            options: Vec::new(),
            status: PermissionStatus::Pending,
            created_at: now,
            resolved_at: None,
            expires_at: None,
        })
        .await
        .unwrap();
    database
        .questions
        .upsert(&AgentQuestionRequest {
            id: "question_restore".to_string(),
            session_id: session.id.clone(),
            turn_id: Some("turn_restore".to_string()),
            tool_call_id: Some("call_restore".to_string()),
            questions: vec![AgentQuestion {
                id: "target".to_string(),
                header: None,
                question: "Which environment?".to_string(),
                options: vec![AgentQuestionOption {
                    label: "staging".to_string(),
                    description: None,
                    value: None,
                }],
                multi_select: false,
                allow_custom: false,
                is_secret: false,
            }],
            answers: Vec::new(),
            status: QuestionStatus::Pending,
            created_at: now,
            resolved_at: None,
            expires_at: None,
        })
        .await
        .unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let restored = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));
    restored.restore_sessions("machine_test").await.unwrap();

    let session = restored.get_session("s_restore").await.expect("session");
    assert_eq!(session.status, SessionStatus::Interrupted);

    for _ in 0..200 {
        let types = database
            .outbox
            .pending(100)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.event.event_type)
            .collect::<Vec<_>>();
        if types.contains(&AgentEventType::TurnInterrupted)
            && types.contains(&AgentEventType::ToolFailed)
            && types.contains(&AgentEventType::PermissionRejected)
            && types.contains(&AgentEventType::QuestionRejected)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let detail = restored.session_detail("s_restore").await.expect("detail");
    assert_eq!(detail.turns[0].status, TurnStatus::Interrupted);
    assert_eq!(detail.tool_calls[0].status, ToolCallStatus::Cancelled);
    assert_eq!(detail.permissions[0].status, PermissionStatus::Cancelled);
    assert!(detail.permissions[0].resolved_at.is_some());
    assert_eq!(detail.questions[0].status, QuestionStatus::Cancelled);
    assert!(detail.questions[0].resolved_at.is_some());

    let types = database
        .outbox
        .pending(100)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.event.event_type)
        .collect::<Vec<_>>();
    assert!(types.contains(&AgentEventType::AgentStatusChanged));
    assert!(types.contains(&AgentEventType::TurnInterrupted));
    assert!(types.contains(&AgentEventType::ToolFailed));
    assert!(types.contains(&AgentEventType::PermissionRejected));
    assert!(types.contains(&AgentEventType::QuestionRejected));
}

#[tokio::test]
async fn terminal_sessions_remain_readable_without_a_runtime_handle() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut session = AgentSession::new(
        "machine_test",
        "mock",
        "mock-agent",
        std::env::temp_dir().to_string_lossy().to_string(),
    );
    session.id = "s_done".to_string();
    session.status = SessionStatus::Completed;
    database.sessions.upsert(&session).await.unwrap();
    database
        .messages
        .insert(&fleet_core_protocol::agent::AgentMessage::new(
            &session.id,
            None,
            fleet_core_protocol::agent::MessageRole::Assistant,
            "old history",
        ))
        .await
        .unwrap();
    let now = fleet_core_common::now();
    database
        .turns
        .upsert(&fleet_core_protocol::agent::AgentTurn {
            id: "turn_stale".to_string(),
            session_id: session.id.clone(),
            status: TurnStatus::Running,
            started_at: now,
            completed_at: None,
            error: None,
        })
        .await
        .unwrap();
    database
        .tool_calls
        .upsert(&fleet_core_protocol::agent::AgentToolCall {
            id: "call_stale".to_string(),
            session_id: session.id.clone(),
            turn_id: Some("turn_stale".to_string()),
            name: "Bash".to_string(),
            status: ToolCallStatus::Running,
            input: serde_json::json!({}),
            output: serde_json::Value::Null,
            error: None,
            started_at: now,
            completed_at: None,
        })
        .await
        .unwrap();
    database
        .permissions
        .upsert(&fleet_core_protocol::agent::AgentPermissionRequest {
            id: "perm_stale".to_string(),
            session_id: session.id.clone(),
            turn_id: Some("turn_stale".to_string()),
            tool_call_id: Some("call_stale".to_string()),
            tool_name: "Bash".to_string(),
            description: "stale".to_string(),
            input: serde_json::json!({}),
            options: Vec::new(),
            status: PermissionStatus::Pending,
            created_at: now,
            resolved_at: None,
            expires_at: None,
        })
        .await
        .unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let restored = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database));
    restored.restore_sessions("machine_test").await.unwrap();

    assert!(
        restored
            .list_sessions()
            .await
            .iter()
            .any(|item| item.id == "s_done"),
        "terminal sessions must stay listed"
    );
    let session = restored.get_session("s_done").await.expect("session");
    assert_eq!(session.status, SessionStatus::Completed);
    let detail = restored.session_detail("s_done").await.expect("detail");
    assert_eq!(detail.messages.len(), 1);
    assert_eq!(detail.messages[0].content, "old history");
    assert_eq!(detail.turns[0].status, TurnStatus::Interrupted);
    assert_eq!(detail.tool_calls[0].status, ToolCallStatus::Cancelled);
    assert_eq!(detail.permissions[0].status, PermissionStatus::Cancelled);
}

#[tokio::test]
async fn delete_session_removes_runtime_handle_and_history() {
    let database = fleet_daemon_storage::DaemonStorage::connect_in_memory()
        .await
        .expect("in-memory database");
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockProvider::new()));
    let runtime = AgentRuntime::new(registry, 4096).with_storage(Arc::new(database.clone()));

    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("delete me"))
        .await
        .expect("send_message");
    collect_until(&mut receiver, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;

    runtime.delete_session(&session.id).await.expect("delete");
    assert!(runtime.get_session(&session.id).await.is_none());
    assert!(runtime.session_detail(&session.id).await.is_none());
    assert!(database.sessions.get(&session.id).await.unwrap().is_none());
    assert!(database
        .messages
        .list(&session.id)
        .await
        .unwrap()
        .is_empty());

    for _ in 0..100 {
        let pending = database.outbox.pending(1_000).await.unwrap();
        if pending.iter().any(|entry| {
            entry.event.event_type == AgentEventType::SessionDeleted
                && entry.event.session_id == session.id
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let pending = database.outbox.pending(1_000).await.unwrap();
    assert!(
        pending.iter().any(|entry| {
            entry.event.event_type == AgentEventType::SessionDeleted
                && entry.event.session_id == session.id
        }),
        "deletion must be recorded in the outbox so replay cannot resurrect the session"
    );
    assert!(
        pending
            .iter()
            .filter(|entry| entry.event.session_id == session.id)
            .all(|entry| entry.event.event_type == AgentEventType::SessionDeleted),
        "older events for a deleted session must not be replayed forever"
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
                session_id: None,
                option_id: None,
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

#[tokio::test]
async fn concurrent_permission_decisions_only_one_wins() {
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
        serde_json::from_value(requested.payload).expect("permission payload");
    let decision = PermissionDecision {
        permission_id: payload.permission.id.clone(),
        session_id: Some(session.id.clone()),
        approve: true,
        option_id: None,
        message: None,
    };
    let (first, second) = tokio::join!(
        runtime.respond_permission(decision.clone()),
        runtime.respond_permission(decision)
    );
    assert!(first.is_ok() != second.is_ok(), "exactly one must win");
    let loser = if first.is_ok() { second } else { first };
    assert!(matches!(loser, Err(RuntimeError::Conflict(_))));
}

#[tokio::test]
async fn mock_question_round_trip_answers_and_returns_to_waiting() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("/question"))
        .await
        .expect("send question command");

    let requested = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::QuestionRequested
    })
    .await;
    let requested: QuestionEventPayload =
        serde_json::from_value(requested.payload).expect("question payload");
    assert_eq!(requested.question.questions.len(), 2);
    assert_eq!(requested.question.questions[0].id, "target");
    assert_eq!(requested.question.status, QuestionStatus::Pending);
    await_status(&runtime, &session.id, SessionStatus::WaitingForQuestion).await;

    runtime
        .respond_question(QuestionDecision {
            question_id: requested.question.id.clone(),
            session_id: Some(session.id.clone()),
            answers: vec![
                AgentQuestionAnswer {
                    id: "target".into(),
                    selected: vec!["staging".into()],
                    custom: None,
                },
                AgentQuestionAnswer {
                    id: "notes".into(),
                    selected: vec![],
                    custom: Some("deploy after lunch".into()),
                },
            ],
            reject: false,
            message: None,
        })
        .await
        .expect("answer question");

    let answered = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::QuestionAnswered
    })
    .await;
    let answered: QuestionEventPayload =
        serde_json::from_value(answered.payload).expect("question payload");
    assert_eq!(answered.question.status, QuestionStatus::Answered);
    await_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;

    let detail = runtime.session_detail(&session.id).await.expect("detail");
    let recorded = detail
        .questions
        .iter()
        .find(|question| question.id == requested.question.id)
        .expect("recorded question");
    assert_eq!(recorded.status, QuestionStatus::Answered);
}

#[tokio::test]
async fn question_decision_rejects_unoffered_option_and_oversized_custom() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("/question"))
        .await
        .expect("send question command");
    let requested = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::QuestionRequested
    })
    .await;
    let requested: QuestionEventPayload =
        serde_json::from_value(requested.payload).expect("question payload");

    let invalid = runtime
        .respond_question(QuestionDecision {
            question_id: requested.question.id.clone(),
            session_id: Some(session.id.clone()),
            answers: vec![
                AgentQuestionAnswer {
                    id: "target".into(),
                    selected: vec!["not-offered".into()],
                    custom: None,
                },
                AgentQuestionAnswer {
                    id: "notes".into(),
                    selected: vec![],
                    custom: None,
                },
            ],
            reject: false,
            message: None,
        })
        .await;
    assert!(matches!(invalid, Err(RuntimeError::InvalidRequest(_))));

    let oversized = runtime
        .respond_question(QuestionDecision {
            question_id: requested.question.id.clone(),
            session_id: Some(session.id.clone()),
            answers: vec![
                AgentQuestionAnswer {
                    id: "target".into(),
                    selected: vec!["staging".into()],
                    custom: None,
                },
                AgentQuestionAnswer {
                    id: "notes".into(),
                    selected: vec![],
                    custom: Some("x".repeat(8 * 1024 + 1)),
                },
            ],
            reject: false,
            message: None,
        })
        .await;
    assert!(matches!(oversized, Err(RuntimeError::InvalidRequest(_))));

    runtime
        .respond_question(QuestionDecision {
            question_id: requested.question.id.clone(),
            session_id: Some(session.id.clone()),
            answers: Vec::new(),
            reject: true,
            message: None,
        })
        .await
        .expect("reject question");
}

#[tokio::test]
async fn concurrent_question_decisions_only_one_wins() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("/question"))
        .await
        .expect("send question command");
    let requested = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::QuestionRequested
    })
    .await;
    let requested: QuestionEventPayload =
        serde_json::from_value(requested.payload).expect("question payload");

    let decision = QuestionDecision {
        question_id: requested.question.id.clone(),
        session_id: Some(session.id.clone()),
        answers: Vec::new(),
        reject: true,
        message: None,
    };
    let (first, second) = tokio::join!(
        runtime.respond_question(decision.clone()),
        runtime.respond_question(decision)
    );
    assert!(first.is_ok() != second.is_ok(), "exactly one must win");
    let loser = if first.is_ok() { second } else { first };
    assert!(matches!(loser, Err(RuntimeError::Conflict(_))));
}

#[tokio::test]
async fn question_decision_with_a_foreign_session_is_rejected() {
    let runtime = runtime_with_mock();
    let (session, mut receiver) = start_mock(&runtime, 1).await;
    runtime
        .send_message(&session.id, send("/question"))
        .await
        .expect("send question command");
    let requested = next_matching(&mut receiver, |event| {
        event.event_type == AgentEventType::QuestionRequested
    })
    .await;
    let requested: QuestionEventPayload =
        serde_json::from_value(requested.payload).expect("question payload");

    let foreign = runtime
        .respond_question(QuestionDecision {
            question_id: requested.question.id.clone(),
            session_id: Some("s_other".into()),
            answers: Vec::new(),
            reject: true,
            message: None,
        })
        .await;
    assert!(matches!(foreign, Err(RuntimeError::QuestionNotFound(_))));

    let detail = runtime.session_detail(&session.id).await.expect("detail");
    assert_eq!(detail.questions[0].status, QuestionStatus::Pending);

    runtime
        .respond_question(QuestionDecision {
            question_id: requested.question.id.clone(),
            session_id: Some(session.id.clone()),
            answers: Vec::new(),
            reject: true,
            message: None,
        })
        .await
        .expect("owner session may reject");
}

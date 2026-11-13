//! OpenCode adapter tests against a deterministic mock server.
//!
//! The mock implements the v2 HTTP + SSE surface used by the adapter, so no
//! model calls, credentials or the real `opencode` binary are required.

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_fleet_agent_runtime::{AgentProvider, AgentRuntime, OpenCodeProvider, ProviderRegistry};
use agent_fleet_common::now;
use agent_fleet_protocol::agent::{
    AgentEventType, AgentStartConfig, MessageCompletedPayload, PermissionDecision, SessionStatus,
    ToolCallStatus, ToolEventPayload,
};
use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::{broadcast, watch, Notify};
use tokio::time::{timeout, Instant};

#[derive(Clone)]
struct MockState {
    tx: broadcast::Sender<String>,
    sequence: Arc<AtomicU64>,
    interrupt: Arc<Notify>,
    permission_tx: watch::Sender<bool>,
    permission_rx: watch::Receiver<bool>,
    replies: Arc<AtomicU64>,
    deleted: Arc<AtomicBool>,
}

async fn start_mock() -> (String, MockState) {
    let (tx, _) = broadcast::channel(512);
    let (permission_tx, permission_rx) = watch::channel(false);
    let state = MockState {
        tx,
        sequence: Arc::new(AtomicU64::new(0)),
        interrupt: Arc::new(Notify::new()),
        permission_tx,
        permission_rx,
        replies: Arc::new(AtomicU64::new(0)),
        deleted: Arc::new(AtomicBool::new(false)),
    };

    let app = Router::new()
        .route("/api/session", post(create_session))
        .route("/api/session/ses_mock/prompt", post(prompt))
        .route("/api/session/ses_mock/interrupt", post(interrupt))
        .route(
            "/api/session/ses_mock/permission/{requestID}/reply",
            post(reply_permission),
        )
        .route("/api/session/ses_mock", delete(delete_session))
        .route("/api/event", get(events))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), state)
}

fn emit(state: &MockState, event_type: &str, data: Value) {
    let sequence = state.sequence.fetch_add(1, Ordering::SeqCst);
    let payload = json!({
        "id": format!("evt_{sequence}"),
        "created": now().timestamp_millis(),
        "type": event_type,
        "data": data,
    });
    let _ = state.tx.send(payload.to_string());
}

async fn create_session() -> Json<Value> {
    Json(json!({
        "data": {
            "id": "ses_mock",
            "title": "mock",
            "projectID": "proj_mock",
            "cost": 0,
            "tokens": {},
            "time": {"created": 0, "updated": 0},
            "location": {"directory": "/tmp"}
        }
    }))
}

async fn prompt(State(state): State<MockState>, Json(body): Json<Value>) -> Json<Value> {
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    tokio::spawn(drive(state, text));
    Json(json!({"data": {"id": "msg_user_1", "sessionID": "ses_mock"}}))
}

async fn interrupt(State(state): State<MockState>) -> Json<Value> {
    state.interrupt.notify_one();
    emit(
        &state,
        "session.execution.interrupted",
        json!({"sessionID": "ses_mock", "reason": "user"}),
    );
    Json(json!({"interrupted": true}))
}

async fn reply_permission(
    State(state): State<MockState>,
    Path(_request_id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    let approve = body
        .get("decision")
        .and_then(Value::as_str)
        .map(|decision| decision != "reject")
        .unwrap_or(false);
    state.replies.fetch_add(1, Ordering::SeqCst);
    let _ = state.permission_tx.send(approve);
    StatusCode::NO_CONTENT
}

async fn delete_session(State(state): State<MockState>) -> StatusCode {
    state.deleted.store(true, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn events(
    State(state): State<MockState>,
) -> Sse<impl futures::Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let mut receiver = state.tx.subscribe();
    let stream = stream! {
        loop {
            match receiver.recv().await {
                Ok(payload) => yield Ok(SseEvent::default().data(payload)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(5)))
}

async fn drive(state: MockState, text: String) {
    emit(
        &state,
        "session.execution.started",
        json!({"sessionID": "ses_mock"}),
    );

    if text.contains("/slow") {
        emit(
            &state,
            "session.text.started",
            json!({"sessionID": "ses_mock", "assistantMessageID": "msg_slow", "ordinal": 0}),
        );
        emit(
            &state,
            "session.text.delta",
            json!({"sessionID": "ses_mock", "assistantMessageID": "msg_slow", "ordinal": 0, "delta": "partial "}),
        );
        state.interrupt.notified().await;
        return;
    }

    if text.contains("/tool") {
        emit(
            &state,
            "session.tool.called",
            json!({
                "sessionID": "ses_mock",
                "assistantMessageID": "msg_tool",
                "id": "tool_1",
                "name": "bash",
                "input": {"command": "echo hi"},
                "executed": false
            }),
        );
        emit(
            &state,
            "session.message.content.updated",
            json!({
                "sessionID": "ses_mock",
                "messageID": "msg_tool",
                "content": [{
                    "type": "tool",
                    "id": "tool_1",
                    "name": "bash",
                    "state": {"status": "running", "input": {"command": "echo hi"}, "metadata": {}},
                    "time": {"created": 0}
                }]
            }),
        );
        emit(
            &state,
            "permission.asked",
            json!({
                "id": "per_1",
                "sessionID": "ses_mock",
                "action": "bash",
                "resources": ["echo hi"],
                "message": "Allow bash to run `echo hi`?",
                "metadata": {}
            }),
        );
        let mut receiver = state.permission_rx.clone();
        let _ = receiver.changed().await;
        let approved = *receiver.borrow();
        if approved {
            emit(
                &state,
                "session.tool.success",
                json!({
                    "sessionID": "ses_mock",
                    "assistantMessageID": "msg_tool",
                    "id": "tool_1",
                    "content": [{"type": "text", "text": "hi\n"}],
                    "executed": true
                }),
            );
            emit(
                &state,
                "session.message.content.updated",
                json!({
                    "sessionID": "ses_mock",
                    "messageID": "msg_tool",
                    "content": [{
                        "type": "tool",
                        "id": "tool_1",
                        "name": "bash",
                        "state": {
                            "status": "completed",
                            "input": {"command": "echo hi"},
                            "content": [{"type": "text", "text": "hi\n"}]
                        },
                        "time": {"created": 0}
                    }]
                }),
            );
        } else {
            emit(
                &state,
                "session.tool.failed",
                json!({
                    "sessionID": "ses_mock",
                    "assistantMessageID": "msg_tool",
                    "id": "tool_1",
                    "error": {"message": "rejected by user"},
                    "executed": true
                }),
            );
        }
        emit(
            &state,
            "session.execution.succeeded",
            json!({"sessionID": "ses_mock"}),
        );
        emit(&state, "session.idle", json!({"sessionID": "ses_mock"}));
        return;
    }

    emit(
        &state,
        "session.text.started",
        json!({"sessionID": "ses_mock", "assistantMessageID": "msg_a1", "ordinal": 0}),
    );
    emit(
        &state,
        "session.text.delta",
        json!({"sessionID": "ses_mock", "assistantMessageID": "msg_a1", "ordinal": 0, "delta": "Hello "}),
    );
    emit(
        &state,
        "session.text.delta",
        json!({"sessionID": "ses_mock", "assistantMessageID": "msg_a1", "ordinal": 0, "delta": "from OpenCode"}),
    );
    emit(
        &state,
        "session.text.ended",
        json!({"sessionID": "ses_mock", "assistantMessageID": "msg_a1", "ordinal": 0, "text": "Hello from OpenCode"}),
    );
    emit(
        &state,
        "session.execution.succeeded",
        json!({"sessionID": "ses_mock"}),
    );
    emit(&state, "session.idle", json!({"sessionID": "ses_mock"}));
}

fn runtime_for(url: &str) -> AgentRuntime {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(OpenCodeProvider::with_server(url, None)));
    AgentRuntime::new(registry, 1024)
}

async fn start(runtime: &AgentRuntime) -> agent_fleet_protocol::agent::AgentSession {
    let directory = tempfile::tempdir().unwrap();
    let config = AgentStartConfig {
        cwd: directory.path().to_string_lossy().to_string(),
        title: Some("opencode test".into()),
        ..Default::default()
    };
    // Keep the temp directory alive for the lifetime of the process; OpenCode
    // only validates it at session creation in this mock.
    std::mem::forget(directory);
    runtime
        .start_session("m1", "opencode", config)
        .await
        .expect("session should start")
}

async fn collect_until(
    receiver: &mut broadcast::Receiver<agent_fleet_protocol::agent::AgentEvent>,
    until: AgentEventType,
    budget: Duration,
) -> Vec<agent_fleet_protocol::agent::AgentEvent> {
    let deadline = Instant::now() + budget;
    let mut events: Vec<agent_fleet_protocol::agent::AgentEvent> = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {until:?}; saw {:?}",
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>()
        );
        match timeout(remaining, receiver.recv()).await {
            Ok(Ok(event)) => {
                let done = event.event_type == until;
                events.push(event);
                if done {
                    return events;
                }
            }
            Ok(Err(error)) => panic!("event stream closed: {error}"),
            Err(_) => continue,
        }
    }
}

async fn wait_for_status(runtime: &AgentRuntime, session_id: &str, expected: SessionStatus) {
    for _ in 0..100 {
        if let Some(session) = runtime.get_session(session_id).await {
            if session.status == expected {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let actual = runtime.get_session(session_id).await.map(|s| s.status);
    panic!("session did not reach {expected:?}, current = {actual:?}");
}

#[tokio::test]
async fn streams_text_and_completes_the_turn() {
    let (url, _state) = start_mock().await;
    let runtime = runtime_for(&url);
    let mut events = runtime.subscribe();
    let session = start(&runtime).await;

    runtime
        .send_message(
            &session.id,
            agent_fleet_protocol::agent::SendMessageRequest {
                content: "hello".into(),
                attachments: vec![],
            },
        )
        .await
        .unwrap();

    let events = collect_until(
        &mut events,
        AgentEventType::TurnCompleted,
        Duration::from_secs(5),
    )
    .await;
    let kinds: Vec<AgentEventType> = events.iter().map(|event| event.event_type).collect();
    assert!(kinds.contains(&AgentEventType::MessageStarted));
    assert!(
        kinds
            .iter()
            .filter(|kind| **kind == AgentEventType::MessageDelta)
            .count()
            >= 2
    );
    assert!(kinds.contains(&AgentEventType::MessageCompleted));
    assert!(kinds.contains(&AgentEventType::AgentStatusChanged));

    let completed = events
        .iter()
        .find(|event| event.event_type == AgentEventType::MessageCompleted)
        .expect("completed message");
    let payload: MessageCompletedPayload =
        serde_json::from_value(completed.payload.clone()).unwrap();
    assert_eq!(payload.message.content, "Hello from OpenCode");

    wait_for_status(&runtime, &session.id, SessionStatus::WaitingForInput).await;
    let detail = runtime.session_detail(&session.id).await.unwrap();
    assert!(detail
        .messages
        .iter()
        .any(|message| message.content.contains("Hello from OpenCode")));
}

#[tokio::test]
async fn approves_tool_permissions_through_the_runtime() {
    let (url, state) = start_mock().await;
    let runtime = runtime_for(&url);
    let mut receiver = runtime.subscribe();
    let session = start(&runtime).await;

    runtime
        .send_message(
            &session.id,
            agent_fleet_protocol::agent::SendMessageRequest {
                content: "/tool".into(),
                attachments: vec![],
            },
        )
        .await
        .unwrap();

    let collected = collect_until(
        &mut receiver,
        AgentEventType::PermissionRequested,
        Duration::from_secs(5),
    )
    .await;
    let requested = collected
        .iter()
        .find(|event| event.event_type == AgentEventType::PermissionRequested)
        .unwrap();
    let permission_id = requested.payload["permission"]["id"]
        .as_str()
        .expect("permission id")
        .to_string();

    runtime
        .respond_permission(PermissionDecision {
            permission_id,
            approve: true,
            message: None,
        })
        .await
        .unwrap();

    let tail = collect_until(
        &mut receiver,
        AgentEventType::TurnCompleted,
        Duration::from_secs(5),
    )
    .await;
    let collected: Vec<_> = collected.into_iter().chain(tail).collect();
    let started = collected
        .iter()
        .find(|event| event.event_type == AgentEventType::ToolStarted)
        .expect("tool started");
    let tool: ToolEventPayload = serde_json::from_value(started.payload.clone()).unwrap();
    assert_eq!(tool.tool.name, "bash");
    assert_eq!(tool.tool.status, ToolCallStatus::Running);

    let completed = collected
        .iter()
        .find(|event| event.event_type == AgentEventType::ToolCompleted)
        .expect("tool completed");
    let tool: ToolEventPayload = serde_json::from_value(completed.payload.clone()).unwrap();
    assert_eq!(tool.tool.status, ToolCallStatus::Completed);
    assert!(tool.tool.output.to_string().contains("hi"));

    assert_eq!(state.replies.load(Ordering::SeqCst), 1);
    let detail = runtime.session_detail(&session.id).await.unwrap();
    assert_eq!(
        detail.permissions[0].status,
        agent_fleet_protocol::agent::PermissionStatus::Approved
    );
}

#[tokio::test]
async fn interrupts_a_running_turn() {
    let (url, _state) = start_mock().await;
    let runtime = runtime_for(&url);
    let mut events = runtime.subscribe();
    let session = start(&runtime).await;

    runtime
        .send_message(
            &session.id,
            agent_fleet_protocol::agent::SendMessageRequest {
                content: "/slow".into(),
                attachments: vec![],
            },
        )
        .await
        .unwrap();

    collect_until(
        &mut events,
        AgentEventType::MessageDelta,
        Duration::from_secs(5),
    )
    .await;
    runtime.interrupt(&session.id).await.unwrap();
    collect_until(
        &mut events,
        AgentEventType::TurnInterrupted,
        Duration::from_secs(5),
    )
    .await;
    wait_for_status(&runtime, &session.id, SessionStatus::Interrupted).await;
}

#[tokio::test]
async fn shutdown_deletes_the_native_session() {
    let (url, state) = start_mock().await;
    let runtime = runtime_for(&url);
    let session = start(&runtime).await;

    runtime.shutdown_session(&session.id).await.unwrap();
    wait_for_status(&runtime, &session.id, SessionStatus::Terminated).await;
    assert!(state.deleted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn reports_missing_binary_and_invalid_cwd() {
    let missing = OpenCodeProvider::new("/definitely/missing/opencode");
    assert!(!missing.info().available);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(missing));
    let runtime = AgentRuntime::new(registry, 64);

    let directory = tempfile::tempdir().unwrap();
    let error = runtime
        .start_session(
            "m1",
            "opencode",
            AgentStartConfig {
                cwd: directory.path().to_string_lossy().to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        agent_fleet_agent_runtime::RuntimeError::ProviderUnavailable(_)
    ));

    let (url, _state) = start_mock().await;
    let runtime = runtime_for(&url);
    let error = runtime
        .start_session(
            "m1",
            "opencode",
            AgentStartConfig {
                cwd: String::new(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        agent_fleet_agent_runtime::RuntimeError::InvalidRequest(_)
    ));
}

#[tokio::test]
#[ignore = "requires the real `opencode` binary and starts a local server"]
async fn real_server_creates_and_deletes_a_session() {
    let available = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("opencode").is_file()))
        .unwrap_or(false);
    if !available {
        eprintln!("opencode binary not found; skipping");
        return;
    }

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(OpenCodeProvider::new("opencode")));
    let runtime = AgentRuntime::new(registry, 256);
    let directory = tempfile::tempdir().unwrap();
    let session = runtime
        .start_session(
            "m1",
            "opencode",
            AgentStartConfig {
                cwd: directory.path().to_string_lossy().to_string(),
                title: Some("agent-fleet real-server smoke".into()),
                ..Default::default()
            },
        )
        .await
        .expect("real opencode server should accept a session");
    assert_eq!(session.status, SessionStatus::Running);
    assert!(session.metadata["provider_session_id"]
        .as_str()
        .unwrap_or_default()
        .starts_with("ses_"));

    runtime.shutdown_session(&session.id).await.unwrap();
    wait_for_status(&runtime, &session.id, SessionStatus::Terminated).await;
}

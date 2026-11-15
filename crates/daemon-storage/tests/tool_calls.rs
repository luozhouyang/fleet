//! Tool-call projections must not lose fields on a sparse completion event.

mod common;

use common::*;
use fleet_core_protocol::agent::{
    AgentToolCall, AgentTurn, SessionStatus, ToolCallStatus, TurnStatus,
};
use fleet_daemon_storage::DaemonStorage;
use serde_json::json;

async fn seed_session_and_turn(db: &DaemonStorage) {
    db.sessions
        .upsert(&session(
            "s_1",
            "m_1",
            ts(1_700_000_000),
            SessionStatus::Running,
            json!({}),
        ))
        .await
        .unwrap();
    db.turns
        .upsert(&AgentTurn {
            id: "turn_1".to_string(),
            session_id: "s_1".to_string(),
            status: TurnStatus::Running,
            started_at: ts(1_700_000_000),
            completed_at: None,
            error: None,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_sparse_completion_preserves_name_input_and_start_time() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_session_and_turn(&db).await;
    let started_at = ts(1_700_000_000);
    let tool = AgentToolCall {
        id: "call_1".to_string(),
        session_id: "s_1".to_string(),
        turn_id: Some("turn_1".to_string()),
        name: "Bash".to_string(),
        status: ToolCallStatus::Running,
        input: json!({"command": "ls"}),
        output: serde_json::Value::Null,
        error: None,
        started_at,
        completed_at: None,
    };
    db.tool_calls.upsert(&tool).await.unwrap();

    let completion = AgentToolCall {
        id: "call_1".to_string(),
        session_id: "s_1".to_string(),
        turn_id: None,
        name: "tool".to_string(),
        status: ToolCallStatus::Completed,
        input: serde_json::Value::Null,
        output: json!({"stdout": "hello\n"}),
        error: None,
        started_at: ts(1_700_000_100),
        completed_at: Some(ts(1_700_000_100)),
    };
    db.tool_calls.upsert(&completion).await.unwrap();

    let stored = db.tool_calls.list("s_1").await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "Bash");
    assert_eq!(stored[0].input["command"], "ls");
    assert_eq!(stored[0].turn_id.as_deref(), Some("turn_1"));
    assert_eq!(stored[0].started_at, started_at);
    assert_eq!(stored[0].status, ToolCallStatus::Completed);
    assert_eq!(stored[0].output["stdout"], "hello\n");
}

#[tokio::test]
async fn a_sparse_completion_event_preserves_the_existing_projection() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_session_and_turn(&db).await;

    let started = fleet_core_protocol::agent::AgentEvent::new(
        "s_1",
        1,
        fleet_core_protocol::agent::AgentEventType::ToolStarted,
        "claude",
    )
    .with_payload(&json!({
        "tool": {
            "id": "call_1",
            "session_id": "s_1",
            "turn_id": "turn_1",
            "name": "Bash",
            "status": "running",
            "input": {"command": "ls"},
            "started_at": "2023-11-14T22:13:20Z"
        }
    }));
    db.record_event(&started).await.unwrap();

    let completed = fleet_core_protocol::agent::AgentEvent::new(
        "s_1",
        2,
        fleet_core_protocol::agent::AgentEventType::ToolCompleted,
        "claude",
    )
    .with_payload(&json!({
        "tool": {
            "id": "call_1",
            "session_id": "s_1",
            "name": "tool",
            "status": "completed",
            "input": null,
            "output": {"stdout": "hello\n"},
            "started_at": "2023-11-14T22:15:00Z",
            "completed_at": "2023-11-14T22:15:00Z"
        }
    }));
    db.record_event(&completed).await.unwrap();

    let stored = db.tool_calls.list("s_1").await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "Bash");
    assert_eq!(stored[0].input["command"], "ls");
    assert_eq!(stored[0].turn_id.as_deref(), Some("turn_1"));
    assert_eq!(stored[0].started_at, ts(1_700_000_000));
    assert_eq!(stored[0].output["stdout"], "hello\n");
}

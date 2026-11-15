mod common;

use common::*;
use fleet_core_common::ApiError;
use fleet_core_protocol::agent::{
    AgentMessage, AgentToolCall, AgentTurn, MessageRole, SessionStatus, ToolCallStatus, TurnStatus,
};
use fleet_daemon_storage::DaemonStorage;
use serde_json::{json, Value};

#[tokio::test]
async fn session_history_roundtrip_and_cascade_delete() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_history",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::WaitingForInput,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();

    let mut turn = AgentTurn::new("s_history");
    turn.id = "turn_1".to_owned();
    turn.status = TurnStatus::Completed;
    turn.completed_at = Some(ts(1_700_000_010));
    db.turns.upsert(&turn).await.unwrap();

    let user = AgentMessage {
        id: "message_user".to_owned(),
        session_id: "s_history".to_owned(),
        turn_id: Some("turn_1".to_owned()),
        role: MessageRole::User,
        content: "hello".to_owned(),
        created_at: ts(1_700_000_001),
    };
    let assistant = AgentMessage {
        id: "message_assistant".to_owned(),
        session_id: "s_history".to_owned(),
        turn_id: Some("turn_1".to_owned()),
        role: MessageRole::Assistant,
        content: "hi".to_owned(),
        created_at: ts(1_700_000_002),
    };
    db.messages.insert(&user).await.unwrap();
    db.messages.insert(&assistant).await.unwrap();

    let mut tool = AgentToolCall {
        id: "tool_1".to_owned(),
        session_id: "s_history".to_owned(),
        turn_id: Some("turn_1".to_owned()),
        name: "Bash".to_owned(),
        status: ToolCallStatus::Running,
        input: json!({"command": "echo hello"}),
        output: Value::Null,
        error: None,
        started_at: ts(1_700_000_003),
        completed_at: None,
    };
    db.tool_calls.upsert(&tool).await.unwrap();

    assert_eq!(db.turns.list("s_history").await.unwrap(), vec![turn]);
    assert_eq!(
        db.messages.list("s_history").await.unwrap(),
        vec![user, assistant]
    );
    assert_eq!(
        db.tool_calls.list("s_history").await.unwrap(),
        vec![tool.clone()]
    );

    tool.status = ToolCallStatus::Completed;
    tool.output = json!({"stdout": "hello\n"});
    tool.completed_at = Some(ts(1_700_000_004));
    db.tool_calls.upsert(&tool).await.unwrap();
    assert_eq!(
        db.tool_calls.list("s_history").await.unwrap(),
        vec![tool.clone()]
    );

    db.tool_calls
        .update_status(
            "s_history",
            "tool_1",
            ToolCallStatus::Failed,
            Some("permission denied"),
            Some(ts(1_700_000_005)),
        )
        .await
        .unwrap();
    let failed = db.tool_calls.list("s_history").await.unwrap();
    assert_eq!(failed[0].status, ToolCallStatus::Failed);
    assert_eq!(failed[0].error.as_deref(), Some("permission denied"));

    assert_eq!(
        db.sessions
            .list_active_for_workspace("ws_1")
            .await
            .unwrap()
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["s_history"]
    );

    let mut closed = s.clone();
    closed.status = SessionStatus::Terminated;
    db.sessions.upsert(&closed).await.unwrap();
    assert!(db
        .sessions
        .list_active_for_workspace("ws_1")
        .await
        .unwrap()
        .is_empty());

    db.sessions.delete("s_history").await.unwrap();
    assert!(db.turns.list("s_history").await.unwrap().is_empty());
    assert!(db.messages.list("s_history").await.unwrap().is_empty());
    assert!(db.tool_calls.list("s_history").await.unwrap().is_empty());
    assert!(db.permissions.list("s_history").await.unwrap().is_empty());
}

#[tokio::test]
async fn session_roundtrip_filters_and_status_update() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();

    let old = session(
        "s_old",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({"provider_session_id": "native-1", "nested": {"step": 1}}),
    );
    let new = session(
        "s_new",
        "m_2",
        ts(1_700_000_200),
        SessionStatus::WaitingForPermission,
        json!({"provider_session_id": "native-2"}),
    );
    db.sessions.upsert(&old).await.unwrap();
    db.sessions.upsert(&new).await.unwrap();

    assert_eq!(db.sessions.get("s_old").await.unwrap().unwrap(), old);

    let all = db.sessions.list(None).await.unwrap();
    assert_eq!(
        all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["s_new", "s_old"]
    );

    let only_m1 = db.sessions.list(Some("m_1")).await.unwrap();
    assert_eq!(only_m1.len(), 1);
    assert_eq!(only_m1[0].id, "s_old");

    let mut updated = old.clone();
    updated.status = SessionStatus::Interrupted;
    updated.updated_at = ts(1_800_000_000);
    updated.last_error = Some(ApiError::new("timeout", "provider stalled"));
    db.sessions.upsert(&updated).await.unwrap();

    let fetched = db.sessions.get("s_old").await.unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Interrupted);
    assert_eq!(fetched.metadata, old.metadata);
    assert_eq!(fetched.last_error, updated.last_error);
    assert_eq!(fetched.created_at, old.created_at);

    db.sessions.delete("s_old").await.unwrap();
    assert!(db.sessions.get("s_old").await.unwrap().is_none());
    assert_eq!(db.sessions.list(None).await.unwrap().len(), 1);
}

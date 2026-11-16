//! Foreign-key behavior for the daemon transcript.

mod common;

use common::*;
use fleet_core_protocol::agent::{
    AgentMessage, AgentToolCall, AgentTurn, MessageRole, PermissionStatus, QuestionStatus,
    SessionStatus, ToolCallStatus, TurnStatus,
};
use fleet_core_protocol::workspace::EntryType;
use fleet_daemon_storage::DaemonStorage;
use serde_json::{json, Value};

async fn seed_transcript(db: &DaemonStorage) {
    db.workspaces
        .upsert(&workspace("ws_1", "m_1", "alpha"))
        .await
        .unwrap();
    let mut session = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    session.workspace_id = Some("ws_1".to_string());
    db.sessions.upsert(&session).await.unwrap();

    db.turns
        .upsert(&AgentTurn {
            id: "turn_1".to_string(),
            session_id: "s_1".to_string(),
            status: TurnStatus::Running,
            started_at: ts(1_700_000_001),
            completed_at: None,
            error: None,
        })
        .await
        .unwrap();
    db.messages
        .insert(&AgentMessage {
            id: "msg_1".to_string(),
            session_id: "s_1".to_string(),
            turn_id: Some("turn_1".to_string()),
            role: MessageRole::User,
            content: "hello".to_string(),
            created_at: ts(1_700_000_002),
        })
        .await
        .unwrap();
    db.tool_calls
        .upsert(&AgentToolCall {
            id: "tool_1".to_string(),
            session_id: "s_1".to_string(),
            turn_id: Some("turn_1".to_string()),
            name: "Bash".to_string(),
            status: ToolCallStatus::Running,
            input: json!({"command": "echo hi"}),
            output: Value::Null,
            error: None,
            started_at: ts(1_700_000_003),
            completed_at: None,
        })
        .await
        .unwrap();
    let mut permission = permission("p_1", "s_1", ts(1_700_000_004), PermissionStatus::Pending);
    permission.turn_id = Some("turn_1".to_string());
    permission.tool_call_id = Some("tool_1".to_string());
    db.permissions.upsert(&permission).await.unwrap();
    let mut question = question_request("q_1", "s_1", ts(1_700_000_005), QuestionStatus::Pending);
    question.turn_id = Some("turn_1".to_string());
    question.tool_call_id = Some("tool_1".to_string());
    db.questions.upsert(&question).await.unwrap();
    db.entries
        .replace("ws_1", "", &[entry("a.txt", "a.txt", EntryType::File)])
        .await
        .unwrap();
}

#[tokio::test]
async fn deleting_a_session_cascades_the_transcript() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_transcript(&db).await;

    sqlx::query("DELETE FROM agent_sessions WHERE id = 's_1'")
        .execute(db.pool())
        .await
        .unwrap();

    for table in [
        "agent_turns",
        "agent_messages",
        "agent_tool_calls",
        "permission_requests",
        "question_requests",
    ] {
        let (count,): (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} must cascade with the session");
    }
    assert_foreign_keys_clean(&db).await;
}

#[tokio::test]
async fn deleting_a_workspace_clears_session_links_and_entries() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_transcript(&db).await;

    sqlx::query("DELETE FROM workspaces WHERE id = 'ws_1'")
        .execute(db.pool())
        .await
        .unwrap();

    let session = db.sessions.get("s_1").await.unwrap().unwrap();
    assert_eq!(session.workspace_id, None);
    assert!(db.entries.list("ws_1", "").await.unwrap().is_empty());
    assert_foreign_keys_clean(&db).await;
}

#[tokio::test]
async fn invalid_transcript_references_are_rejected() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let error = sqlx::query(
        r#"
        INSERT INTO agent_messages (id, session_id, turn_id, role, content, created_at)
        VALUES ('bad', 'missing_session', NULL, 'user', 'x', '2024-01-01T00:00:00Z')
        "#,
    )
    .execute(db.pool())
    .await
    .expect_err("missing session must be rejected");
    assert!(error.to_string().contains("FOREIGN KEY"));
}

async fn assert_foreign_keys_clean(db: &DaemonStorage) {
    let (violations,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_foreign_key_check")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(violations, 0);
}

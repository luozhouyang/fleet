//! Transactional outbox and cursor behavior.

mod common;

use common::*;
use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentMessage, MessageCompletedPayload, MessageDeltaPayload,
    MessageRole, SessionStatus,
};
use fleet_daemon_storage::DaemonStorage;
use serde_json::json;

fn message_event(session_id: &str, message_id: &str, text: &str) -> AgentEvent {
    let message = AgentMessage {
        id: message_id.to_string(),
        session_id: session_id.to_string(),
        turn_id: Some("turn_1".to_string()),
        role: MessageRole::User,
        content: text.to_string(),
        created_at: ts(1_700_000_010),
    };
    AgentEvent::new(session_id, 1, AgentEventType::MessageCompleted, "mock")
        .with_payload(&MessageCompletedPayload { message })
}

#[tokio::test]
async fn outbox_is_ordered_acked_and_pruned() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();

    let first = message_event("s_1", "msg_1", "one");
    let second = message_event("s_1", "msg_2", "two");
    let third = message_event("s_1", "msg_3", "three");
    assert_eq!(db.outbox.append(&first).await.unwrap(), 1);
    assert_eq!(db.outbox.append(&second).await.unwrap(), 2);
    assert_eq!(db.outbox.append(&third).await.unwrap(), 3);

    let pending = db.outbox.pending(10).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(pending[0].event, first);

    assert_eq!(db.outbox.ack(2).await.unwrap(), 2);
    assert_eq!(db.outbox.cursor().await.unwrap(), 2);
    assert_eq!(db.outbox.len().await.unwrap(), 1);
    assert_eq!(db.outbox.pending(10).await.unwrap()[0].sequence, 3);

    // Cursors never move backwards.
    assert_eq!(db.outbox.ack(1).await.unwrap(), 2);
    assert_eq!(db.outbox.cursor().await.unwrap(), 2);

    // Duplicate event ids are idempotent.
    assert_eq!(db.outbox.append(&third).await.unwrap(), 3);
    assert_eq!(db.outbox.len().await.unwrap(), 1);
}

#[tokio::test]
async fn event_store_updates_projection_and_outbox_together() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();

    let event = message_event("s_1", "msg_1", "hello");
    db.record_event(&event).await.unwrap();

    let messages = db.messages.list("s_1").await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "hello");
    assert_eq!(db.outbox.pending(10).await.unwrap().len(), 1);

    let ephemeral = AgentEvent::new("s_1", 2, AgentEventType::MessageDelta, "mock").with_payload(
        &MessageDeltaPayload {
            message_id: "msg_1".to_string(),
            text: "ignored".to_string(),
            index: None,
        },
    );
    db.record_event(&ephemeral).await.unwrap();
    assert_eq!(db.outbox.pending(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn session_created_event_creates_metadata_and_outbox_atomically() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_new",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Starting,
        json!({}),
    );
    let event =
        AgentEvent::new("s_new", 1, AgentEventType::SessionCreated, "mock").with_payload(&s);

    db.record_event(&event).await.unwrap();

    assert_eq!(db.sessions.get("s_new").await.unwrap().unwrap(), s);
    assert_eq!(db.outbox.pending(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn stale_events_cannot_resurrect_a_deleted_session() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    db.sessions.delete("s_1").await.unwrap();

    db.record_event(&message_event("s_1", "msg_1", "late"))
        .await
        .unwrap();

    assert!(db.messages.list("s_1").await.unwrap().is_empty());
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn deletion_events_are_kept_even_when_the_session_row_is_gone() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    db.sessions.delete("s_1").await.unwrap();

    let event = AgentEvent::new("s_1", 1, AgentEventType::SessionDeleted, "mock")
        .with_payload(&json!({"reason": "deleted"}));
    db.record_event(&event).await.unwrap();

    assert!(db.sessions.get("s_1").await.unwrap().is_none());
    let pending = db.outbox.pending(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event.event_type, AgentEventType::SessionDeleted);
}

#[tokio::test]
async fn unacknowledged_events_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");

    {
        let db = DaemonStorage::connect(&path).await.unwrap();
        let s = session(
            "s_1",
            "m_1",
            ts(1_700_000_000),
            SessionStatus::Running,
            json!({}),
        );
        db.sessions.upsert(&s).await.unwrap();
        db.record_event(&message_event("s_1", "msg_1", "persist"))
            .await
            .unwrap();
    }

    let db = DaemonStorage::connect(&path).await.unwrap();
    let pending = db.outbox.pending(10).await.unwrap();
    assert_eq!(pending.len(), 1, "pending rows must survive a restart");
    assert_eq!(db.outbox.cursor().await.unwrap(), 0);

    db.outbox.ack(1).await.unwrap();
    drop(db);

    let db = DaemonStorage::connect(&path).await.unwrap();
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
    assert_eq!(db.outbox.cursor().await.unwrap(), 1);
    assert_eq!(db.messages.list("s_1").await.unwrap().len(), 1);
}

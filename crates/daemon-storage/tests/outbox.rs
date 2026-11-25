//! Transactional outbox and cursor behavior.

mod common;

use common::*;
use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentMessage, MessageCompletedPayload, MessageDeltaPayload,
    MessageRole, PermissionEventPayload, PermissionStatus, QuestionEventPayload, QuestionStatus,
    SessionStatus,
};
use fleet_daemon_storage::DaemonStorage;
use serde_json::json;

fn message_event(session_id: &str, message_id: &str, text: &str) -> AgentEvent {
    let message = AgentMessage {
        id: message_id.to_string(),
        session_id: session_id.to_string(),
        turn_id: None,
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

    assert_eq!(db.outbox.ack(1).await.unwrap(), 1);
    assert_eq!(db.outbox.cursor().await.unwrap(), 1);
    assert_eq!(db.outbox.len().await.unwrap(), 2);
    assert_eq!(db.outbox.ack(2).await.unwrap(), 2);
    assert_eq!(db.outbox.cursor().await.unwrap(), 2);
    assert_eq!(db.outbox.len().await.unwrap(), 1);
    assert_eq!(db.outbox.pending(10).await.unwrap()[0].sequence, 3);

    // Cursors never move backwards.
    assert_eq!(db.outbox.ack(1).await.unwrap(), 2);
    assert_eq!(db.outbox.cursor().await.unwrap(), 2);
    assert!(db.outbox.ack(4).await.is_err());

    // Duplicate event ids are idempotent.
    assert_eq!(db.outbox.append(&third).await.unwrap(), 3);
    assert_eq!(db.outbox.len().await.unwrap(), 1);
}

#[tokio::test]
async fn out_of_order_acknowledgements_wait_for_the_gap() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    for (message, text) in [("msg_1", "one"), ("msg_2", "two"), ("msg_3", "three")] {
        db.outbox
            .append(&message_event("s_1", message, text))
            .await
            .unwrap();
    }

    // The admin applied 2 before 1 (for example after a replay).
    assert_eq!(db.outbox.ack(2).await.unwrap(), 0);
    assert_eq!(db.outbox.cursor().await.unwrap(), 0);
    assert_eq!(db.outbox.len().await.unwrap(), 3);
    // Row 2 is already acknowledged; replaying it would only waste a delivery
    // slot that a still-unacknowledged row needs.
    assert_eq!(
        db.outbox
            .pending(10)
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );

    // Filling the gap advances the cursor over both acknowledgements.
    assert_eq!(db.outbox.ack(1).await.unwrap(), 2);
    assert_eq!(db.outbox.len().await.unwrap(), 1);
    assert_eq!(db.outbox.pending(10).await.unwrap()[0].sequence, 3);

    assert_eq!(db.outbox.ack(3).await.unwrap(), 3);
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn acknowledged_rows_do_not_fill_the_replay_window() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    for index in 0..10 {
        db.outbox
            .append(&message_event(
                "s_1",
                &format!("msg_{index}"),
                &format!("message {index}"),
            ))
            .await
            .unwrap();
    }

    // Acknowledge everything above the gap out of order.
    for sequence in 2..=10 {
        db.outbox.ack(sequence).await.unwrap();
    }
    assert_eq!(db.outbox.cursor().await.unwrap(), 0);
    // The replay window contains only the missing row, not nine acknowledged
    // rows that would starve the backlog.
    assert_eq!(
        db.outbox
            .pending(3)
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1]
    );

    assert_eq!(db.outbox.ack(1).await.unwrap(), 10);
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn batch_acknowledgements_advance_only_the_contiguous_prefix() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    for index in 0..6 {
        db.outbox
            .append(&message_event(
                "s_1",
                &format!("msg_{index}"),
                &format!("message {index}"),
            ))
            .await
            .unwrap();
    }

    // 1 and 3..6 applied, 2 still in flight: the cursor stops at 1.
    assert_eq!(db.outbox.ack_batch(&[6, 3, 1, 4, 5]).await.unwrap(), 1);
    assert_eq!(db.outbox.cursor().await.unwrap(), 1);
    assert_eq!(
        db.outbox
            .pending(10)
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![2]
    );

    // Filling the gap lets one batch confirm the whole backlog.
    assert_eq!(db.outbox.ack_batch(&[2]).await.unwrap(), 6);
    assert!(db.outbox.pending(10).await.unwrap().is_empty());

    // Pruned sequences arriving late are ignored instead of failing the batch.
    assert_eq!(db.outbox.ack_batch(&[3, 4]).await.unwrap(), 6);
}

#[tokio::test]
async fn unparseable_rows_are_discarded_without_stalling_later_events() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    db.outbox
        .append(&message_event("s_1", "msg_1", "one"))
        .await
        .unwrap();
    db.outbox
        .append(&message_event("s_1", "msg_2", "two"))
        .await
        .unwrap();
    sqlx::query("UPDATE event_outbox SET payload_json = '{not json' WHERE sequence = 2")
        .execute(db.pool())
        .await
        .unwrap();

    let pending = db.outbox.pending(10).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(db.outbox.ack(1).await.unwrap(), 2);
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
    assert_eq!(db.outbox.cursor().await.unwrap(), 2);
}

#[tokio::test]
async fn oversized_rows_are_discarded_without_stalling_later_events() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    db.outbox
        .append(&message_event("s_1", "msg_1", "one"))
        .await
        .unwrap();
    db.outbox
        .append(&message_event("s_1", "msg_2", "two"))
        .await
        .unwrap();
    let oversized = "x".repeat(5 * 1024 * 1024);
    sqlx::query("UPDATE event_outbox SET payload_json = ?1 WHERE sequence = 2")
        .bind(&oversized)
        .execute(db.pool())
        .await
        .unwrap();

    assert_eq!(
        db.outbox
            .pending(10)
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(db.outbox.ack(1).await.unwrap(), 2);
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
}

#[test]
fn terminal_and_activity_events_are_live_only() {
    use fleet_daemon_storage::outbox::is_durable_event;

    assert!(!is_durable_event(AgentEventType::TerminalOutput));
    assert!(!is_durable_event(AgentEventType::ActivityChanged));
    assert!(!is_durable_event(AgentEventType::UsageUpdated));
    assert!(!is_durable_event(AgentEventType::MessageDelta));
    assert!(is_durable_event(AgentEventType::PlanUpdated));
    assert!(is_durable_event(AgentEventType::Notice));
    assert!(is_durable_event(AgentEventType::ToolCompleted));
}

#[tokio::test]
async fn deleting_a_session_discards_its_older_pending_events() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    for (message, text) in [("msg_1", "one"), ("msg_2", "two")] {
        db.outbox
            .append(&message_event("s_1", message, text))
            .await
            .unwrap();
    }
    let deletion = AgentEvent::new("s_1", 3, AgentEventType::SessionDeleted, "mock")
        .with_payload(&json!({"reason": "deleted"}));
    let deletion_sequence = db.outbox.append(&deletion).await.unwrap();
    assert_eq!(deletion_sequence, 3);

    db.outbox
        .discard_session_before("s_1", deletion_sequence)
        .await
        .unwrap();

    let pending = db.outbox.pending(10).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(db.outbox.ack(3).await.unwrap(), 3);
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn session_deletion_is_atomic_and_prunes_older_pending_events() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();
    db.outbox
        .append(&message_event("s_1", "msg_1", "one"))
        .await
        .unwrap();
    db.outbox
        .append(&message_event("s_1", "msg_2", "two"))
        .await
        .unwrap();

    let deletion = AgentEvent::new("s_1", 3, AgentEventType::SessionDeleted, "mock")
        .with_payload(&json!({"reason": "deleted"}));
    let sequence = db.record_session_deleted(&deletion).await.unwrap();
    assert_eq!(sequence, 3);
    assert!(db.sessions.get("s_1").await.unwrap().is_none());
    assert_eq!(db.outbox.cursor().await.unwrap(), 2);

    let pending = db.outbox.pending(10).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(db.outbox.ack(3).await.unwrap(), 3);
    assert!(db.outbox.pending(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_out_of_order_acknowledgements_converge() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();

    for index in 0..50 {
        db.outbox
            .append(&message_event(
                "s_1",
                &format!("msg_{index}"),
                &format!("message {index}"),
            ))
            .await
            .unwrap();
    }

    let mut tasks = Vec::new();
    for sequence in (1..=50).rev() {
        let outbox = db.outbox.clone();
        tasks.push(tokio::spawn(async move { outbox.ack(sequence).await }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }

    assert_eq!(db.outbox.cursor().await.unwrap(), 50);
    assert!(db.outbox.pending(100).await.unwrap().is_empty());
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
    let path = dir.path().join("fleet-daemon.db");

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

#[tokio::test]
async fn dangling_turn_and_tool_references_are_nulled_instead_of_failing() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();

    // Agents name turn and tool ids in their own space; an id the projection
    // has never seen must not fail the foreign key and stall the outbox.
    let mut question = question_request("q_1", "s_1", ts(1_700_000_010), QuestionStatus::Pending);
    question.turn_id = Some("turn_unknown".to_string());
    question.tool_call_id = Some("call_unknown".to_string());
    let event = AgentEvent::new("s_1", 1, AgentEventType::QuestionRequested, "mock").with_payload(
        &QuestionEventPayload {
            question: question.clone(),
        },
    );
    let cursor = db.record_event(&event).await.unwrap();
    assert!(cursor.is_some(), "the question event must be durable");
    let stored = db.questions.list("s_1").await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].turn_id, None);
    assert_eq!(stored[0].tool_call_id, None);

    let mut request = permission("p_1", "s_1", ts(1_700_000_011), PermissionStatus::Pending);
    request.turn_id = Some("turn_unknown".to_string());
    request.tool_call_id = Some("call_unknown".to_string());
    let event = AgentEvent::new("s_1", 2, AgentEventType::PermissionRequested, "mock")
        .with_payload(&PermissionEventPayload {
            permission: request.clone(),
        });
    let cursor = db.record_event(&event).await.unwrap();
    assert!(cursor.is_some(), "the permission event must be durable");
    let stored = db.permissions.list("s_1").await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].turn_id, None);
    assert_eq!(stored[0].tool_call_id, None);
}

#[tokio::test]
async fn busy_outbox_writes_are_classified_as_retryable() {
    use std::time::Duration;

    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::SqlitePool;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("daemon.db");
    let db = DaemonStorage::connect(&path).await.unwrap();
    let s = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    db.sessions.upsert(&s).await.unwrap();

    // Hold the write lock from a second connection. The runtime retries a
    // failure like this with backoff instead of downgrading the event.
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .busy_timeout(Duration::from_millis(50));
    let blocker = SqlitePool::connect_with(options).await.unwrap();
    let mut guard = blocker.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *guard)
        .await
        .unwrap();

    let error = db
        .record_event(&message_event("s_1", "msg_1", "blocked"))
        .await
        .expect_err("the outbox write must fail while another writer holds the lock");
    assert!(
        error.is_retryable(),
        "busy/locked writes must be retryable, got: {error}"
    );

    sqlx::query("ROLLBACK").execute(&mut *guard).await.unwrap();
    drop(guard);
    blocker.close().await;

    let cursor = db
        .record_event(&message_event("s_1", "msg_1", "blocked"))
        .await
        .unwrap();
    assert_eq!(cursor, Some(1), "the retried event gets its cursor");
}

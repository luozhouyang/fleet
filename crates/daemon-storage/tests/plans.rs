//! Agent plans are durable: a `agent.plan.updated` event projects the latest
//! plan so a reloading client can restore the plan row.

mod common;

use common::*;
use fleet_core_protocol::agent::{AgentEvent, AgentEventType, PlanEntryStatus};
use fleet_daemon_storage::DaemonStorage;
use serde_json::json;

async fn seed_session(db: &DaemonStorage) {
    db.sessions
        .upsert(&session(
            "s_1",
            "m_1",
            ts(1_700_000_000),
            fleet_core_protocol::agent::SessionStatus::Running,
            json!({}),
        ))
        .await
        .unwrap();
}

fn plan_event(sequence: u64, entries: serde_json::Value) -> AgentEvent {
    AgentEvent::new("s_1", sequence, AgentEventType::PlanUpdated, "acp").with_payload(&json!({
        "plan": {
            "id": "plan_1",
            "session_id": "s_1",
            "entries": entries,
            "updated_at": format!("2023-11-14T22:13:{sequence:02}Z")
        }
    }))
}

#[tokio::test]
async fn plan_updates_replace_the_stored_plan() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_session(&db).await;

    let first = plan_event(
        10,
        json!([{"content": "inspect", "status": "in_progress", "priority": "high"}]),
    );
    db.record_event(&first).await.unwrap();

    let stored = db.plans.get("s_1").await.unwrap().expect("plan stored");
    assert_eq!(stored.id, "plan_1");
    assert_eq!(stored.entries.len(), 1);
    assert_eq!(stored.entries[0].content, "inspect");
    assert_eq!(stored.entries[0].status, PlanEntryStatus::InProgress);

    let second = plan_event(
        20,
        json!([
            {"content": "inspect", "status": "completed", "priority": "high"},
            {"content": "implement", "status": "pending", "priority": "medium"}
        ]),
    );
    db.record_event(&second).await.unwrap();

    let stored = db.plans.get("s_1").await.unwrap().expect("plan stored");
    assert_eq!(stored.entries.len(), 2);
    assert_eq!(stored.entries[0].status, PlanEntryStatus::Completed);
    assert_eq!(stored.entries[1].content, "implement");
}

#[tokio::test]
async fn deleting_a_session_removes_its_plan() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_session(&db).await;
    db.record_event(&plan_event(
        1,
        json!([{"content": "x", "status": "pending"}]),
    ))
    .await
    .unwrap();

    let deleted = AgentEvent::new("s_1", 2, AgentEventType::SessionDeleted, "acp");
    db.record_event(&deleted).await.unwrap();

    assert!(db.plans.get("s_1").await.unwrap().is_none());
}

#[tokio::test]
async fn a_plan_for_an_unknown_session_is_dropped() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let event = plan_event(1, json!([{"content": "x", "status": "pending"}]));
    assert!(db.record_event(&event).await.unwrap().is_none());
    assert!(db.plans.get("s_1").await.unwrap().is_none());
}

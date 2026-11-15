mod common;

use common::*;
use fleet_admin_storage::AdminStorage;
use fleet_core_common::ApiError;
use fleet_core_protocol::agent::SessionStatus;
use serde_json::json;

#[tokio::test]
async fn session_roundtrip_filters_and_status_update() {
    let db = AdminStorage::connect_in_memory().await.unwrap();

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

mod common;

use common::*;
use fleet_admin_storage::AdminStorage;
use fleet_core_common::ApiError;
use fleet_core_protocol::agent::SessionStatus;
use serde_json::json;

#[tokio::test]
async fn session_roundtrip_filters_and_status_update() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    for id in ["m_1", "m_2"] {
        db.machines.upsert(&machine(id, id)).await.unwrap();
    }
    db.workspaces
        .upsert(&workspace("ws_1", "m_1", "alpha"))
        .await
        .unwrap();
    db.workspaces
        .upsert(&workspace("ws_2", "m_2", "beta"))
        .await
        .unwrap();

    let old = session(
        "s_old",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({"provider_session_id": "native-1", "nested": {"step": 1}}),
    );
    let mut new = session(
        "s_new",
        "m_2",
        ts(1_700_000_200),
        SessionStatus::WaitingForPermission,
        json!({"provider_session_id": "native-2"}),
    );
    new.workspace_id = Some("ws_2".to_string());
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

#[tokio::test]
async fn manual_title_survives_daemon_upserts_until_cleared() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    db.machines.upsert(&machine("m_1", "m_1")).await.unwrap();

    let mut created = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({}),
    );
    created.title = Some("provider title".into());
    db.sessions.upsert(&created).await.unwrap();

    assert!(db
        .sessions
        .update_title("s_1", Some("My session"))
        .await
        .unwrap());

    let mut snapshot = created.clone();
    snapshot.title = Some("provider title".into());
    snapshot.updated_at = ts(1_700_000_300);
    db.sessions.upsert(&snapshot).await.unwrap();
    assert_eq!(
        db.sessions
            .get("s_1")
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("My session")
    );
    assert_eq!(
        db.sessions
            .overridden_titles("m_1")
            .await
            .unwrap()
            .get("s_1")
            .map(String::as_str),
        Some("My session")
    );

    assert!(db.sessions.update_title("s_1", None).await.unwrap());
    assert!(db
        .sessions
        .overridden_titles("m_1")
        .await
        .unwrap()
        .is_empty());
    let mut later = snapshot;
    later.title = Some("auto title".into());
    later.updated_at = ts(1_700_000_400);
    db.sessions.upsert(&later).await.unwrap();
    assert_eq!(
        db.sessions
            .get("s_1")
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("auto title")
    );
    assert!(!db
        .sessions
        .update_title("missing", Some("x"))
        .await
        .unwrap());
}

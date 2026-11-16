mod common;

use common::*;
use fleet_core_protocol::agent::{PermissionStatus, SessionStatus};
use fleet_daemon_storage::DaemonStorage;
use serde_json::json;

async fn seed_sessions(db: &DaemonStorage) {
    for id in ["s_1", "s_2", "session_1"] {
        db.sessions
            .upsert(&session(
                id,
                "m_1",
                ts(1_700_000_000),
                SessionStatus::Running,
                json!({}),
            ))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn permission_listing_and_resolution_roundtrip() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_sessions(&db).await;

    let first = permission("p_1", "s_1", ts(1_700_000_100), PermissionStatus::Pending);
    let second = permission("p_2", "s_1", ts(1_700_000_050), PermissionStatus::Approved);
    let other = permission("p_3", "s_2", ts(1_700_000_075), PermissionStatus::Pending);
    db.permissions.upsert(&first).await.unwrap();
    db.permissions.upsert(&second).await.unwrap();
    db.permissions.upsert(&other).await.unwrap();

    assert_eq!(db.permissions.get("p_1").await.unwrap().unwrap(), first);

    let all_for_session = db.permissions.list("s_1").await.unwrap();
    assert_eq!(
        all_for_session
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        vec!["p_1", "p_2"]
    );

    let pending = db.permissions.list_pending("s_1").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "p_1");
    assert_eq!(db.permissions.list("s_2").await.unwrap().len(), 1);

    let mut resolved = first.clone();
    resolved.status = PermissionStatus::Approved;
    resolved.resolved_at = Some(ts(1_800_000_000));
    db.permissions.upsert(&resolved).await.unwrap();

    let fetched = db.permissions.get("p_1").await.unwrap().unwrap();
    assert_eq!(fetched.status, PermissionStatus::Approved);
    assert_eq!(fetched.resolved_at, Some(ts(1_800_000_000)));
    assert_eq!(fetched.input, first.input);
    assert!(db.permissions.list_pending("s_1").await.unwrap().is_empty());
}

#[tokio::test]
async fn pending_permission_can_only_be_resolved_once() {
    use fleet_core_protocol::agent::{AgentPermissionRequest, PermissionStatus};

    let db = DaemonStorage::connect_in_memory().await.unwrap();
    seed_sessions(&db).await;
    let permission = AgentPermissionRequest {
        id: "perm_1".into(),
        session_id: "session_1".into(),
        turn_id: None,
        tool_call_id: None,
        tool_name: "Bash".into(),
        description: "run".into(),
        input: json!({}),
        options: Vec::new(),
        status: PermissionStatus::Pending,
        created_at: fleet_core_common::now(),
        resolved_at: None,
        expires_at: None,
    };
    db.permissions.upsert(&permission).await.unwrap();

    assert!(db
        .permissions
        .resolve_pending("perm_1", PermissionStatus::Approved)
        .await
        .unwrap());
    assert!(!db
        .permissions
        .resolve_pending("perm_1", PermissionStatus::Rejected)
        .await
        .unwrap());
    let stored = db.permissions.get("perm_1").await.unwrap().unwrap();
    assert_eq!(stored.status, PermissionStatus::Approved);

    db.permissions.revert_pending("perm_1").await.unwrap();
    assert_eq!(
        db.permissions.get("perm_1").await.unwrap().unwrap().status,
        PermissionStatus::Pending
    );
}

//! Foreign-key behavior for the admin database.

mod common;

use common::*;
use fleet_admin_storage::AdminStorage;
use fleet_core_protocol::agent::{SessionStatus};
use serde_json::json;

#[tokio::test]
async fn deleting_a_machine_cascades_children() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    db.machines
        .upsert(&machine("m_1", "machine"))
        .await
        .unwrap();
    db.machine_orgs.assign("m_1", "org_default").await.unwrap();
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
    // Bypass the repository so SQLite, not application code, performs the
    // cascade.
    sqlx::query("DELETE FROM machines WHERE id = 'm_1'")
        .execute(db.pool())
        .await
        .unwrap();

    assert!(db.machines.get("m_1").await.unwrap().is_none());
    assert!(db.workspaces.get("ws_1").await.unwrap().is_none());
    assert!(db.sessions.get("s_1").await.unwrap().is_none());
    assert_eq!(db.machine_orgs.get("m_1").await.unwrap(), None);
    assert_foreign_keys_clean(&db).await;
}

#[tokio::test]
async fn deleting_a_workspace_keeps_sessions_without_a_workspace() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    db.machines
        .upsert(&machine("m_1", "machine"))
        .await
        .unwrap();
    db.workspaces
        .upsert(&workspace("ws_1", "m_1", "alpha"))
        .await
        .unwrap();
    let mut session = session(
        "s_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Completed,
        json!({}),
    );
    session.workspace_id = Some("ws_1".to_string());
    db.sessions.upsert(&session).await.unwrap();

    sqlx::query("DELETE FROM workspaces WHERE id = 'ws_1'")
        .execute(db.pool())
        .await
        .unwrap();

    let stored = db.sessions.get("s_1").await.unwrap().unwrap();
    assert_eq!(stored.workspace_id, None);
    assert_foreign_keys_clean(&db).await;
}

#[tokio::test]
async fn invalid_references_are_rejected() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let error = sqlx::query(
        r#"
        INSERT INTO workspaces (
            id, machine_id, project_id, name, root_path, is_git_repository,
            created_at, updated_at
        ) VALUES (
            'ws_bad', 'missing_machine', NULL, 'bad', '/tmp/bad', 0,
            '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z'
        )
        "#,
    )
    .execute(db.pool())
    .await
    .expect_err("missing machine must be rejected");
    assert!(error.to_string().contains("FOREIGN KEY"));
}

async fn assert_foreign_keys_clean(db: &AdminStorage) {
    let (violations,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_foreign_key_check")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(violations, 0);
}

//! Foreign-key behavior for the admin database.

mod common;

use common::*;
use fleet_admin_storage::AdminStorage;
use fleet_core_protocol::agent::SessionStatus;
use fleet_core_protocol::authz::OrganizationKind;
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
async fn deleting_an_organization_keeps_machines_but_leaves_them_unowned() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let org = db
        .organizations
        .create(OrganizationKind::Team, "Team", None)
        .await
        .unwrap();
    db.machines
        .upsert(&machine("m_1", "machine"))
        .await
        .unwrap();
    db.machine_orgs.assign("m_1", &org.id).await.unwrap();
    db.workspaces
        .upsert(&workspace("ws_1", "m_1", "alpha"))
        .await
        .unwrap();

    // Detaching is explicit and leaves the machine row in place.
    assert_eq!(db.machine_orgs.clear_org(&org.id).await.unwrap(), 1);
    assert_eq!(db.machine_orgs.get("m_1").await.unwrap(), None);
    assert_eq!(
        db.machine_orgs.unowned().await.unwrap(),
        vec!["m_1".to_string()]
    );

    // Even a direct hard delete must not cascade into machines or workspaces;
    // the foreign key only clears ownership.
    db.machine_orgs.assign("m_1", &org.id).await.unwrap();
    sqlx::query("DELETE FROM organizations WHERE id = ?1")
        .bind(&org.id)
        .execute(db.pool())
        .await
        .unwrap();

    assert!(db.machines.get("m_1").await.unwrap().is_some());
    assert!(db.workspaces.get("ws_1").await.unwrap().is_some());
    assert_eq!(db.machine_orgs.get("m_1").await.unwrap(), None);
    assert_foreign_keys_clean(&db).await;
}

#[tokio::test]
async fn purging_an_expired_organization_keeps_machines_and_audit() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let org = db
        .organizations
        .create(OrganizationKind::Team, "Team", None)
        .await
        .unwrap();
    db.machines
        .upsert(&machine("m_1", "machine"))
        .await
        .unwrap();
    db.machine_orgs.assign("m_1", &org.id).await.unwrap();
    db.workspaces
        .upsert(&workspace("ws_1", "m_1", "alpha"))
        .await
        .unwrap();
    let user = db
        .users
        .create(Some("ada@example.com"), "Ada", None, None, None, None)
        .await
        .unwrap();
    db.memberships
        .upsert(
            &org.id,
            &user.id,
            fleet_core_protocol::authz::UserRole::Member,
            None,
        )
        .await
        .unwrap();

    let timestamp = fleet_core_common::now();
    // Accepted invitations and used pairing codes survive the soft delete, so
    // the purge cascade is what removes them.
    sqlx::query(
        "INSERT INTO org_invitations (id, org_id, email, role, token_hash, created_by, \
         created_at, expires_at, accepted_at) VALUES ('invite_1', ?1, 'old@example.com', \
         'member', 'invite-hash', 'admin', ?2, ?2, ?2)",
    )
    .bind(&org.id)
    .bind(timestamp.to_rfc3339())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pairing_codes (id, code_hash, org_id, created_by, expires_at, used_at, \
         created_at) VALUES ('pair_used', 'used-hash', ?1, 'admin', ?2, ?2, ?2)",
    )
    .bind(&org.id)
    .bind(timestamp.to_rfc3339())
    .execute(db.pool())
    .await
    .unwrap();
    db.audit
        .append(
            "admin",
            Some(&org.id),
            "org.create",
            &org.id,
            json!({"name": "Team"}),
        )
        .await
        .unwrap();

    let deletion = db
        .organizations
        .soft_delete_detaching(&org.id)
        .await
        .unwrap()
        .expect("organization must be deleted");
    assert_eq!(deletion.detached_machines, vec!["m_1".to_string()]);
    assert_eq!(deletion.deleted_memberships, 1);
    assert_eq!(db.machine_orgs.get("m_1").await.unwrap(), None);
    assert!(db
        .memberships
        .get(&org.id, &user.id)
        .await
        .unwrap()
        .is_none());

    let purged = db
        .organizations
        .purge_expired(timestamp + chrono::Duration::days(30), "retention")
        .await
        .unwrap();
    assert_eq!(purged.len(), 1);
    assert_eq!(purged[0].id, org.id);
    assert_eq!(purged[0].name, "Team");

    assert!(db.organizations.get(&org.id).await.unwrap().is_none());
    assert!(db
        .memberships
        .get(&org.id, &user.id)
        .await
        .unwrap()
        .is_none());
    let (invitations,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM org_invitations WHERE org_id = ?1")
            .bind(&org.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(invitations, 0);
    let (codes,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pairing_codes WHERE org_id = ?1")
        .bind(&org.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(codes, 0, "used pairing codes cascade with the organization");

    // Machines, workspaces, sessions and audit rows are never deleted.
    assert!(db.machines.get("m_1").await.unwrap().is_some());
    assert!(db.workspaces.get("ws_1").await.unwrap().is_some());
    let (audit_rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE org_id = ?1")
        .bind(&org.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(audit_rows, 1);
    let (purge_audits,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'org.purge' AND subject = ?1",
    )
    .bind(&org.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(purge_audits, 1, "purge and audit commit together");
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

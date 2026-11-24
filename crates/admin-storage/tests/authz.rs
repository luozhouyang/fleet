//! Organization, user and machine-ownership storage tests.

mod common;

use common::machine as machine_fixture;
use fleet_admin_storage::AdminStorage;
use fleet_core_protocol::authz::{OrganizationKind, UserRole, DEFAULT_ORG_ID};

#[tokio::test]
async fn organizations_users_and_machine_ownership_roundtrip() {
    let db = AdminStorage::connect_in_memory().await.unwrap();

    let org = db
        .organizations
        .create(OrganizationKind::Team, "Acme", None)
        .await
        .unwrap();
    assert_eq!(db.organizations.get(&org.id).await.unwrap().unwrap(), org);

    let default = db.organizations.ensure_default().await.unwrap();
    assert_eq!(default.id, DEFAULT_ORG_ID);
    assert_eq!(db.organizations.list().await.unwrap().len(), 2);

    let issued = fleet_core_common::now();
    let user = db
        .users
        .create(
            Some("ada@example.com"),
            "Ada",
            None,
            Some("hash-a"),
            Some(issued),
            Some(issued + chrono::Duration::days(30)),
        )
        .await
        .unwrap();
    assert_eq!(db.users.get(&user.id).await.unwrap().unwrap(), user);
    assert_eq!(
        db.users.by_email("ada@example.com").await.unwrap().unwrap(),
        user
    );
    assert_eq!(
        db.users.by_token_hash("hash-a").await.unwrap().unwrap(),
        user
    );
    let rotated_at = issued + chrono::Duration::days(1);
    assert!(db
        .users
        .update_token(&user.id, "hash-b", rotated_at, None)
        .await
        .unwrap());
    assert!(db.users.by_token_hash("hash-a").await.unwrap().is_none());
    let updated = db.users.by_token_hash("hash-b").await.unwrap().unwrap();
    assert_eq!(updated.id, user.id);
    assert_eq!(updated.token_issued_at, Some(rotated_at));
    assert_eq!(updated.token_expires_at, None);
    assert_eq!(db.users.list().await.unwrap().len(), 1);

    // Membership, not the user row, is what scopes an account to an
    // organization.
    db.memberships
        .upsert(&org.id, &user.id, UserRole::Admin, None)
        .await
        .unwrap();
    let members = db.memberships.members_of(&org.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, user.id);
    assert_eq!(members[0].role, UserRole::Admin);
    assert!(db
        .memberships
        .members_of(DEFAULT_ORG_ID)
        .await
        .unwrap()
        .is_empty());

    db.machines
        .upsert(&machine_fixture("m_1", "machine-one"))
        .await
        .unwrap();
    db.machine_orgs.assign("m_1", &org.id).await.unwrap();
    db.machine_orgs.ensure("m_1", DEFAULT_ORG_ID).await.unwrap();
    assert_eq!(
        db.machine_orgs.get("m_1").await.unwrap(),
        Some(org.id.clone())
    );
    assert_eq!(
        db.machine_orgs.ids_for_org(&org.id).await.unwrap(),
        vec!["m_1".to_string()]
    );
    assert!(db
        .machine_orgs
        .ids_for_org(DEFAULT_ORG_ID)
        .await
        .unwrap()
        .is_empty());

    assert!(db.users.delete(&user.id).await.unwrap());
    assert!(db.users.get(&user.id).await.unwrap().is_none());
    assert!(!db.users.delete(&user.id).await.unwrap());
}

#[tokio::test]
async fn user_token_expiry_columns_round_trip_and_null_means_never() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let issued = fleet_core_common::now();

    let expiring = db
        .users
        .create(
            Some("expiring@example.com"),
            "Expiring",
            None,
            Some("hash-expiring"),
            Some(issued),
            Some(issued + chrono::Duration::days(7)),
        )
        .await
        .unwrap();
    assert_eq!(expiring.token_issued_at, Some(issued));
    assert_eq!(
        expiring.token_expires_at,
        Some(issued + chrono::Duration::days(7))
    );

    let fetched = db.users.get(&expiring.id).await.unwrap().unwrap();
    assert_eq!(fetched, expiring);
    let by_token = db
        .users
        .by_token_hash("hash-expiring")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_token, expiring);
    let listed = db.users.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].token_expires_at, expiring.token_expires_at);

    // NULL expiry rows (existing tokens and the never-expires choice) read as
    // `None` rather than an error.
    sqlx::query("UPDATE users SET token_issued_at = NULL, token_expires_at = NULL WHERE id = ?1")
        .bind(&expiring.id)
        .execute(db.pool())
        .await
        .unwrap();
    let legacy = db.users.get(&expiring.id).await.unwrap().unwrap();
    assert_eq!(legacy.token_issued_at, None);
    assert_eq!(legacy.token_expires_at, None);

    assert!(db
        .users
        .update_token(&expiring.id, "hash-rotated", issued, None)
        .await
        .unwrap());
    let rotated = db
        .users
        .by_token_hash("hash-rotated")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rotated.token_issued_at, Some(issued));
    assert_eq!(rotated.token_expires_at, None);
    assert!(!db
        .users
        .update_token("user_missing", "hash", issued, None)
        .await
        .unwrap());
}

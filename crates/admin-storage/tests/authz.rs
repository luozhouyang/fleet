//! Organization, user and machine-ownership storage tests.

mod common;

use common::machine as machine_fixture;
use fleet_admin_storage::AdminStorage;
use fleet_core_protocol::authz::{UserRole, DEFAULT_ORG_ID};

#[tokio::test]
async fn organizations_users_and_machine_ownership_roundtrip() {
    let db = AdminStorage::connect_in_memory().await.unwrap();

    let org = db.organizations.create("Acme").await.unwrap();
    assert_eq!(db.organizations.get(&org.id).await.unwrap().unwrap(), org);

    let default = db.organizations.ensure_default().await.unwrap();
    assert_eq!(default.id, DEFAULT_ORG_ID);
    assert_eq!(db.organizations.list().await.unwrap().len(), 2);

    let user = db
        .users
        .create(&org.id, "Ada", UserRole::Admin, "hash-a")
        .await
        .unwrap();
    assert_eq!(db.users.get(&user.id).await.unwrap().unwrap(), user);
    assert_eq!(
        db.users.by_token_hash("hash-a").await.unwrap().unwrap(),
        user
    );
    assert!(db
        .users
        .update_token_hash(&user.id, "hash-b")
        .await
        .unwrap());
    assert!(db.users.by_token_hash("hash-a").await.unwrap().is_none());
    let updated = db.users.by_token_hash("hash-b").await.unwrap().unwrap();
    assert_eq!(updated.id, user.id);
    assert_eq!(updated.org_id, user.org_id);
    assert_eq!(db.users.list(Some(&org.id)).await.unwrap().len(), 1);
    assert!(db
        .users
        .list(Some(DEFAULT_ORG_ID))
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

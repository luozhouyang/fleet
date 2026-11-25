//! Membership queries scoped to live organizations.

use fleet_admin_storage::AdminStorage;
use fleet_core_common::now;
use fleet_core_protocol::authz::{OrganizationKind, UserRole};

#[tokio::test]
async fn memberships_of_deleted_organizations_are_not_listed_for_the_user() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let org = db
        .organizations
        .create(OrganizationKind::Team, "Gone", None)
        .await
        .unwrap();
    let user = db
        .users
        .create(Some("ada@example.com"), "Ada", None, None, None, None)
        .await
        .unwrap();
    db.memberships
        .upsert(&org.id, &user.id, UserRole::Owner, None)
        .await
        .unwrap();

    assert_eq!(db.memberships.for_user(&user.id).await.unwrap().len(), 1);
    db.organizations
        .soft_delete_detaching(&org.id)
        .await
        .unwrap();
    assert!(db.memberships.for_user(&user.id).await.unwrap().is_empty());

    // A write racing the delete leaves the row behind; it must stay invisible
    // so the organization cannot authenticate members again.
    sqlx::query(
        "INSERT INTO memberships (org_id, user_id, role, created_at, updated_at) \
         VALUES (?1, ?2, 'owner', ?3, ?3)",
    )
    .bind(&org.id)
    .bind(&user.id)
    .bind(now().to_rfc3339())
    .execute(db.pool())
    .await
    .unwrap();
    assert!(db.memberships.for_user(&user.id).await.unwrap().is_empty());
}

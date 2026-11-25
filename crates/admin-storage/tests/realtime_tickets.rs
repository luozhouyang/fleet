//! Retention for single-use realtime event-socket tickets.

use fleet_admin_storage::AdminStorage;
use fleet_core_common::now;
use fleet_core_protocol::authz::OrganizationKind;

#[tokio::test]
async fn pruning_removes_expired_and_old_consumed_tickets() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let timestamp = now();
    let org = db
        .organizations
        .create(OrganizationKind::Team, "Org", None)
        .await
        .unwrap();
    let user = db
        .users
        .create(Some("reader@example.com"), "Reader", None, None, None, None)
        .await
        .unwrap();
    for (hash, expires_at) in [
        ("valid", timestamp + chrono::Duration::hours(1)),
        ("used", timestamp + chrono::Duration::hours(1)),
        ("fresh_used", timestamp + chrono::Duration::hours(1)),
        ("expired", timestamp - chrono::Duration::hours(2)),
    ] {
        db.realtime_tickets
            .create(hash, &user.id, &org.id, None, expires_at)
            .await
            .unwrap();
    }

    // One ticket was consumed long ago, one just now.
    assert!(db.realtime_tickets.consume("used").await.unwrap().is_some());
    sqlx::query("UPDATE realtime_tickets SET consumed_at = ?1 WHERE ticket_hash = 'used'")
        .bind((timestamp - chrono::Duration::hours(2)).to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db
        .realtime_tickets
        .consume("fresh_used")
        .await
        .unwrap()
        .is_some());

    assert_eq!(db.realtime_tickets.prune().await.unwrap(), 2);

    let (remaining,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM realtime_tickets")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        remaining, 2,
        "only live and recently consumed tickets remain"
    );
    assert!(db
        .realtime_tickets
        .consume("valid")
        .await
        .unwrap()
        .is_some());
}

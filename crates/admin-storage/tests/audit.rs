use fleet_admin_storage::AdminStorage;
use serde_json::{json, Value};

#[tokio::test]
async fn append_audit_log_persists_details() {
    let db = AdminStorage::connect_in_memory().await.unwrap();

    db.audit
        .append(
            "admin",
            Some("org_default"),
            "machine.delete",
            "machine_1",
            json!({"reason": "retired", "count": 1}),
        )
        .await
        .unwrap();
    db.audit
        .append(
            "admin",
            Some("org_default"),
            "workspace.remove",
            "ws_1",
            Value::Null,
        )
        .await
        .unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_logs")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count.0, 2);

    let row: (String, String, String, String, String) = sqlx::query_as(
        "SELECT actor, action, subject, details_json, created_at FROM audit_logs \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(row.0, "admin");
    assert_eq!(row.1, "machine.delete");
    assert_eq!(row.2, "machine_1");
    assert_eq!(row.3, json!({"reason": "retired", "count": 1}).to_string());
    assert!(!row.4.is_empty());

    let entries = db.audit.list(None, 10).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].action, "workspace.remove");
    assert_eq!(entries[0].details, Value::Null);
    assert_eq!(entries[1].action, "machine.delete");
    assert_eq!(entries[1].actor, "admin");
    assert_eq!(entries[1].subject, "machine_1");
    assert_eq!(entries[1].details, json!({"reason": "retired", "count": 1}));

    // The limit is clamped into 1..=500.
    assert_eq!(db.audit.list(None, 0).await.unwrap().len(), 1);
    assert_eq!(db.audit.list(None, 10_000).await.unwrap().len(), 2);

    // Organization filtering happens before LIMIT and keeps newer entries
    // from another organization from crowding out the caller's history.
    db.audit
        .append("Ada", Some("org_a"), "git.commit", "ws_a", json!({}))
        .await
        .unwrap();
    db.audit
        .append("Bob", Some("org_b"), "git.commit", "ws_b", json!({}))
        .await
        .unwrap();
    let org_a = db.audit.list(Some("org_a"), 10).await.unwrap();
    assert_eq!(org_a.len(), 1);
    assert_eq!(org_a[0].subject, "ws_a");
}

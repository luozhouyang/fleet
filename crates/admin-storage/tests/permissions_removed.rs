use fleet_admin_storage::AdminStorage;

#[tokio::test]
async fn permission_requests_table_is_removed() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'permission_requests'",
    )
    .fetch_optional(db.pool())
    .await
    .unwrap();
    assert_eq!(exists, None);
}

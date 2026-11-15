use fleet_daemon_storage::DaemonStorage;

#[tokio::test]
async fn in_memory_connect_runs_migrations_once() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    db.migrate().await.unwrap();

    let table: (String,) = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'agent_sessions'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(table.0, "agent_sessions");

    assert!(db.sessions.list(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn file_connect_creates_parents_and_uses_wal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/data/agent.db");

    let db = DaemonStorage::connect(&path).await.unwrap();
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "database must be private to its service user");
    }

    let journal_mode: (String,) = sqlx::query_as("PRAGMA journal_mode")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(journal_mode.0.to_ascii_lowercase(), "wal");

    let tables: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_tool_calls'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(tables.0, 1);
}

#[tokio::test]
async fn legacy_shared_schema_is_adopted_and_admin_tables_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("CREATE TABLE _sqlx_migrations (version INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    for table in [
        "machines",
        "organizations",
        "users",
        "audit_logs",
        "machine_orgs",
    ] {
        let sql = format!("CREATE TABLE {table} (id TEXT PRIMARY KEY)");
        sqlx::query(&sql).execute(&pool).await.unwrap();
    }
    sqlx::query(
        "CREATE TABLE agent_messages (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, turn_id TEXT, role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_messages (id, session_id, role, content, created_at) VALUES ('msg_1', 's_1', 'user', 'keep me', '2024-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let db = DaemonStorage::connect(&path).await.unwrap();
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains("pre-split"))
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1, "legacy schema must be backed up");
    let role: (String,) = sqlx::query_as("SELECT value FROM storage_meta WHERE key = 'role'")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(role.0, "daemon");
    for table in [
        "machines",
        "organizations",
        "users",
        "audit_logs",
        "machine_orgs",
    ] {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1")
                .bind(table)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, 0, "{table} must not exist in the daemon database");
    }
    let content: (String,) =
        sqlx::query_as("SELECT content FROM agent_messages WHERE id = 'msg_1'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(content.0, "keep me");
}

#[tokio::test]
async fn rejects_an_admin_database_without_modifying_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent-admin.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("CREATE TABLE storage_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO storage_meta (key, value) VALUES ('role', 'admin')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE machines (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, name) VALUES ('m_1', 'keep me')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let error = DaemonStorage::connect(&path).await.err().unwrap();
    assert!(
        error.to_string().contains("role is 'admin'"),
        "unexpected error: {error}"
    );

    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let role: (String,) = sqlx::query_as("SELECT value FROM storage_meta WHERE key = 'role'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(role.0, "admin");
    let name: (String,) = sqlx::query_as("SELECT name FROM machines WHERE id = 'm_1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name.0, "keep me");
}

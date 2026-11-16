use fleet_admin_storage::AdminStorage;

#[tokio::test]
async fn in_memory_connect_runs_migrations_once() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
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
    let path = dir.path().join("nested/data/fleet-admin.db");

    let db = AdminStorage::connect(&path).await.unwrap();
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
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'audit_logs'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(tables.0, 1);
}

#[tokio::test]
async fn legacy_shared_schema_is_adopted_and_daemon_tables_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fleet-admin.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("CREATE TABLE _sqlx_migrations (version INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE agent_messages (id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE agent_turns (id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE agent_tool_calls (id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE workspace_entries (id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let db = AdminStorage::connect(&path).await.unwrap();
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
    assert_eq!(role.0, "admin");
    for table in [
        "agent_messages",
        "agent_turns",
        "agent_tool_calls",
        "workspace_entries",
    ] {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1")
                .bind(table)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, 0, "{table} must not exist in the admin database");
    }
}

#[tokio::test]
async fn rejects_a_daemon_database_without_modifying_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fleet-daemon.db");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("CREATE TABLE storage_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO storage_meta (key, value) VALUES ('role', 'daemon')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE agent_messages (id TEXT PRIMARY KEY, content TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agent_messages (id, content) VALUES ('msg_1', 'keep me')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let error = AdminStorage::connect(&path).await.err().unwrap();
    assert!(
        error.to_string().contains("role is 'daemon'"),
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
    assert_eq!(role.0, "daemon");
    let content: (String,) =
        sqlx::query_as("SELECT content FROM agent_messages WHERE id = 'msg_1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(content.0, "keep me");
}

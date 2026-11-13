//! End-to-end tests for the SQLite storage crate.

use agent_fleet_common::{new_id, ApiError, Timestamp};
use agent_fleet_protocol::agent::{
    AgentCapabilities, AgentPermissionRequest, AgentSession, PermissionStatus, ProviderInfo,
    ProviderKind, SessionStatus,
};
use agent_fleet_protocol::git::GitFileStatus;
use agent_fleet_protocol::machine::{Machine, MachineStatus};
use agent_fleet_protocol::workspace::{EntryType, Workspace, WorkspaceEntry, WorkspaceSummary};
use agent_fleet_storage::Database;
use chrono::{Duration, TimeZone, Utc};
use serde_json::{json, Value};

fn ts(seconds: i64) -> Timestamp {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .expect("valid fixed timestamp")
}

fn machine(id: &str, name: &str) -> Machine {
    Machine {
        id: id.to_owned(),
        name: name.to_owned(),
        hostname: format!("{name}.local"),
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        daemon_version: "0.1.0".to_owned(),
        protocol_version: 1,
        status: MachineStatus::Online,
        created_at: ts(1_700_000_000),
        updated_at: ts(1_700_000_100),
        connected_at: Some(ts(1_700_000_050)),
        last_seen_at: Some(ts(1_700_000_090)),
        providers: vec![ProviderInfo {
            id: "mock".to_owned(),
            kind: ProviderKind::Mock,
            display_name: "Mock Agent".to_owned(),
            version: Some("0.1.0".to_owned()),
            available: true,
            capabilities: AgentCapabilities::full(),
            models: vec!["mock-fast".to_owned(), "mock-smart".to_owned()],
        }],
        workspaces: vec![WorkspaceSummary {
            id: "ws_1".to_owned(),
            name: "alpha".to_owned(),
            root_path: "/tmp/alpha".to_owned(),
            is_git_repository: true,
        }],
        workspace_count: 1,
        public_key: Some("public-key".to_owned()),
    }
}

fn workspace(id: &str, machine_id: &str, name: &str) -> Workspace {
    Workspace {
        id: id.to_owned(),
        machine_id: machine_id.to_owned(),
        project_id: Some("project_1".to_owned()),
        name: name.to_owned(),
        root_path: format!("/tmp/{name}"),
        is_git_repository: true,
        created_at: ts(1_700_000_000),
        updated_at: ts(1_700_000_100),
    }
}

fn session(
    id: &str,
    machine_id: &str,
    created_at: Timestamp,
    status: SessionStatus,
    metadata: Value,
) -> AgentSession {
    AgentSession {
        id: id.to_owned(),
        machine_id: machine_id.to_owned(),
        workspace_id: Some("ws_1".to_owned()),
        provider: "mock".to_owned(),
        agent_type: "mock-agent".to_owned(),
        status,
        cwd: "/tmp/alpha".to_owned(),
        title: Some(format!("session {id}")),
        created_at,
        updated_at: created_at + Duration::seconds(5),
        last_error: None,
        metadata,
    }
}

fn permission(
    id: &str,
    session_id: &str,
    created_at: Timestamp,
    status: PermissionStatus,
) -> AgentPermissionRequest {
    AgentPermissionRequest {
        id: id.to_owned(),
        session_id: session_id.to_owned(),
        turn_id: Some("turn_1".to_owned()),
        tool_call_id: Some("call_1".to_owned()),
        tool_name: "Bash".to_owned(),
        description: "Run a command".to_owned(),
        input: json!({"command": "ls"}),
        status,
        created_at,
        resolved_at: None,
        expires_at: Some(created_at + Duration::minutes(5)),
    }
}

fn entry(path: &str, name: &str, entry_type: EntryType) -> WorkspaceEntry {
    WorkspaceEntry {
        path: path.to_owned(),
        name: name.to_owned(),
        entry_type,
        size: Some(42),
        modified_at: Some(ts(1_700_000_123)),
        git_status: Some(GitFileStatus::Modified),
        ignored: true,
    }
}

fn paths(entries: &[WorkspaceEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.path.clone()).collect()
}

#[tokio::test]
async fn in_memory_connect_runs_migrations_once() {
    let db = Database::connect_in_memory().await.unwrap();
    db.migrate().await.unwrap();

    let table: (String,) = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'agent_sessions'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(table.0, "agent_sessions");

    assert!(db.list_machines().await.unwrap().is_empty());
}

#[tokio::test]
async fn file_connect_creates_parents_and_uses_wal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/data/agent.db");

    let db = Database::connect(&path).await.unwrap();
    assert!(path.exists());

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
async fn machine_roundtrip_status_list_and_delete() {
    let db = Database::connect_in_memory().await.unwrap();

    let alpha = machine("m_1", "alpha");
    let beta = machine("m_2", "beta");
    db.upsert_machine(&beta).await.unwrap();
    db.upsert_machine(&alpha).await.unwrap();

    let fetched = db.get_machine("m_1").await.unwrap().unwrap();
    assert_eq!(fetched, alpha);

    let listed = db.list_machines().await.unwrap();
    assert_eq!(
        listed.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );

    db.update_machine_status(
        "m_1",
        MachineStatus::Degraded,
        None,
        Some(ts(1_800_000_000)),
    )
    .await
    .unwrap();

    let updated = db.get_machine("m_1").await.unwrap().unwrap();
    assert_eq!(updated.status, MachineStatus::Degraded);
    assert_eq!(updated.connected_at, alpha.connected_at);
    assert_eq!(updated.last_seen_at, Some(ts(1_800_000_000)));
    assert!(updated.updated_at > alpha.updated_at);

    // Upsert must replace mutable fields while preserving the nested providers.
    let mut renamed = alpha.clone();
    renamed.name = "alpha-renamed".to_owned();
    renamed.providers[0].models.push("extra-model".to_owned());
    renamed.updated_at = ts(1_800_000_100);
    db.upsert_machine(&renamed).await.unwrap();
    let replaced = db.get_machine("m_1").await.unwrap().unwrap();
    assert_eq!(replaced.name, "alpha-renamed");
    assert_eq!(replaced.providers[0].models.len(), 3);
    assert_eq!(replaced.created_at, alpha.created_at);

    db.delete_machine("m_1").await.unwrap();
    assert!(db.get_machine("m_1").await.unwrap().is_none());
    assert_eq!(db.list_machines().await.unwrap().len(), 1);
}

#[tokio::test]
async fn malformed_json_columns_degrade_to_defaults() {
    let db = Database::connect_in_memory().await.unwrap();
    let machine = machine("m_1", "alpha");
    db.upsert_machine(&machine).await.unwrap();

    sqlx::query("UPDATE machines SET providers_json = ?1, workspaces_json = ?2 WHERE id = ?3")
        .bind("{not json")
        .bind("also not json")
        .bind("m_1")
        .execute(db.pool())
        .await
        .unwrap();

    let fetched = db.get_machine("m_1").await.unwrap().unwrap();
    assert!(fetched.providers.is_empty());
    assert!(fetched.workspaces.is_empty());
    assert_eq!(fetched.workspace_count, 0);
}

#[tokio::test]
async fn workspace_upsert_list_and_delete() {
    let db = Database::connect_in_memory().await.unwrap();

    let alpha = workspace("ws_1", "m_1", "alpha");
    let beta = workspace("ws_2", "m_2", "beta");
    db.upsert_workspace(&beta).await.unwrap();
    db.upsert_workspace(&alpha).await.unwrap();

    assert_eq!(db.get_workspace("ws_1").await.unwrap().unwrap(), alpha);

    let all = db.list_workspaces(None).await.unwrap();
    assert_eq!(
        all.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );

    let only_m1 = db.list_workspaces(Some("m_1")).await.unwrap();
    assert_eq!(only_m1.len(), 1);
    assert_eq!(only_m1[0].id, "ws_1");

    let mut renamed = alpha.clone();
    renamed.name = "alpha-renamed".to_owned();
    renamed.root_path = "/tmp/renamed".to_owned();
    renamed.updated_at = ts(1_800_000_000);
    db.upsert_workspace(&renamed).await.unwrap();
    assert_eq!(db.get_workspace("ws_1").await.unwrap().unwrap(), renamed);

    db.delete_workspace("ws_2").await.unwrap();
    assert!(db.get_workspace("ws_2").await.unwrap().is_none());
    assert_eq!(db.list_workspaces(None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn session_roundtrip_filters_and_status_update() {
    let db = Database::connect_in_memory().await.unwrap();

    let old = session(
        "s_old",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::Running,
        json!({"provider_session_id": "native-1", "nested": {"step": 1}}),
    );
    let new = session(
        "s_new",
        "m_2",
        ts(1_700_000_200),
        SessionStatus::WaitingForPermission,
        json!({"provider_session_id": "native-2"}),
    );
    db.upsert_session(&old).await.unwrap();
    db.upsert_session(&new).await.unwrap();

    assert_eq!(db.get_session("s_old").await.unwrap().unwrap(), old);

    let all = db.list_sessions(None).await.unwrap();
    assert_eq!(
        all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["s_new", "s_old"]
    );

    let only_m1 = db.list_sessions(Some("m_1")).await.unwrap();
    assert_eq!(only_m1.len(), 1);
    assert_eq!(only_m1[0].id, "s_old");

    let mut updated = old.clone();
    updated.status = SessionStatus::Interrupted;
    updated.updated_at = ts(1_800_000_000);
    updated.last_error = Some(ApiError::new("timeout", "provider stalled"));
    db.upsert_session(&updated).await.unwrap();

    let fetched = db.get_session("s_old").await.unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Interrupted);
    assert_eq!(fetched.metadata, old.metadata);
    assert_eq!(fetched.last_error, updated.last_error);
    assert_eq!(fetched.created_at, old.created_at);

    db.delete_session("s_old").await.unwrap();
    assert!(db.get_session("s_old").await.unwrap().is_none());
    assert_eq!(db.list_sessions(None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn permission_listing_and_resolution_roundtrip() {
    let db = Database::connect_in_memory().await.unwrap();

    let first = permission("p_1", "s_1", ts(1_700_000_100), PermissionStatus::Pending);
    let second = permission("p_2", "s_1", ts(1_700_000_050), PermissionStatus::Approved);
    let other = permission("p_3", "s_2", ts(1_700_000_075), PermissionStatus::Pending);
    db.upsert_permission(&first).await.unwrap();
    db.upsert_permission(&second).await.unwrap();
    db.upsert_permission(&other).await.unwrap();

    assert_eq!(db.get_permission("p_1").await.unwrap().unwrap(), first);

    let all_for_session = db.list_permissions("s_1").await.unwrap();
    assert_eq!(
        all_for_session
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        vec!["p_1", "p_2"]
    );

    let pending = db.list_pending_permissions("s_1").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "p_1");
    assert_eq!(db.list_permissions("s_2").await.unwrap().len(), 1);

    let mut resolved = first.clone();
    resolved.status = PermissionStatus::Approved;
    resolved.resolved_at = Some(ts(1_800_000_000));
    db.upsert_permission(&resolved).await.unwrap();

    let fetched = db.get_permission("p_1").await.unwrap().unwrap();
    assert_eq!(fetched.status, PermissionStatus::Approved);
    assert_eq!(fetched.resolved_at, Some(ts(1_800_000_000)));
    assert_eq!(fetched.input, first.input);
    assert!(db.list_pending_permissions("s_1").await.unwrap().is_empty());
}

#[tokio::test]
async fn replace_list_and_delete_entries() {
    let db = Database::connect_in_memory().await.unwrap();
    let ws = "ws_1";
    let other = "ws_2";

    db.replace_entries(
        ws,
        "src",
        &[
            entry("src/b.rs", "b.rs", EntryType::File),
            entry("src/a.rs", "a.rs", EntryType::File),
        ],
    )
    .await
    .unwrap();
    db.replace_entries(
        ws,
        "docs",
        &[entry("docs/readme.md", "readme.md", EntryType::File)],
    )
    .await
    .unwrap();
    db.replace_entries(
        ws,
        "",
        &[
            entry("root.txt", "root.txt", EntryType::File),
            entry("src_extra", "src_extra", EntryType::Directory),
        ],
    )
    .await
    .unwrap();
    db.replace_entries(
        other,
        "src",
        &[entry("src/other.rs", "other.rs", EntryType::File)],
    )
    .await
    .unwrap();

    let src = db.list_entries(ws, "src").await.unwrap();
    assert_eq!(paths(&src), vec!["src/a.rs", "src/b.rs"]);
    assert_eq!(src[0].name, "a.rs");
    assert_eq!(src[0].entry_type, EntryType::File);
    assert_eq!(src[0].size, Some(42));
    assert_eq!(src[0].modified_at, Some(ts(1_700_000_123)));
    assert_eq!(src[0].git_status, Some(GitFileStatus::Modified));
    assert!(src[0].ignored);

    // Replacing one parent leaves its siblings and other parents untouched.
    db.replace_entries(ws, "src", &[entry("src/c.rs", "c.rs", EntryType::File)])
        .await
        .unwrap();
    assert_eq!(
        paths(&db.list_entries(ws, "src").await.unwrap()),
        vec!["src/c.rs"]
    );
    assert_eq!(db.list_entries(ws, "docs").await.unwrap().len(), 1);
    assert_eq!(db.list_entries(ws, "").await.unwrap().len(), 2);

    // A nested parent is removed by deleting its ancestor subtree.
    db.replace_entries(
        ws,
        "src/nested",
        &[entry("src/nested/x.rs", "x.rs", EntryType::File)],
    )
    .await
    .unwrap();

    db.delete_entries_under(ws, "src").await.unwrap();
    assert!(db.list_entries(ws, "src").await.unwrap().is_empty());
    assert!(db.list_entries(ws, "src/nested").await.unwrap().is_empty());
    assert_eq!(db.list_entries(ws, "docs").await.unwrap().len(), 1);
    assert_eq!(
        paths(&db.list_entries(ws, "").await.unwrap()),
        vec!["root.txt", "src_extra"]
    );
    assert_eq!(db.list_entries(other, "src").await.unwrap().len(), 1);
}

#[tokio::test]
async fn delete_entries_under_escapes_like_wildcards() {
    let db = Database::connect_in_memory().await.unwrap();
    let ws = "ws_escape";

    db.replace_entries(
        ws,
        "",
        &[
            entry("a%b/x", "x", EntryType::File),
            entry("axxb/x", "x", EntryType::File),
            entry("a_b/y", "y", EntryType::File),
            entry("axb/y", "y", EntryType::File),
        ],
    )
    .await
    .unwrap();

    db.delete_entries_under(ws, "a%b").await.unwrap();
    assert_eq!(
        paths(&db.list_entries(ws, "").await.unwrap()),
        vec!["a_b/y", "axb/y", "axxb/x"]
    );

    db.delete_entries_under(ws, "a_b").await.unwrap();
    assert_eq!(
        paths(&db.list_entries(ws, "").await.unwrap()),
        vec!["axb/y", "axxb/x"]
    );
}

#[tokio::test]
async fn append_audit_log_persists_details() {
    let db = Database::connect_in_memory().await.unwrap();

    db.append_audit_log(
        "admin",
        "machine.delete",
        "machine_1",
        json!({"reason": "retired", "count": 1}),
    )
    .await
    .unwrap();
    db.append_audit_log("admin", "workspace.remove", "ws_1", Value::Null)
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
}

#[tokio::test]
async fn concurrent_reads_do_not_fail() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::connect(dir.path().join("agent.db"))
        .await
        .unwrap();

    for index in 0..3 {
        let machine = machine(&format!("m_{index}"), &format!("machine-{index}"));
        db.upsert_machine(&machine).await.unwrap();
    }

    let mut handles = Vec::new();
    for task in 0..4 {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            for round in 0..25 {
                let machines = db.list_machines().await?;
                assert_eq!(machines.len(), 3, "task {task} round {round}");
            }
            Ok::<(), agent_fleet_storage::StorageError>(())
        }));
    }

    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Writes still work after the concurrent read burst.
    let machine = machine(&new_id("machine"), "after-reads");
    db.upsert_machine(&machine).await.unwrap();
    assert_eq!(db.list_machines().await.unwrap().len(), 4);
}

//! Upgrade from a populated pre-multi-organization (v10) database.

use std::path::Path;

use fleet_admin_storage::AdminStorage;
use fleet_core_protocol::authz::{OrganizationKind, UserRole};

const CREATED_AT: &str = "2026-01-01T00:00:00+00:00";
const EXPIRES_AT: &str = "2026-02-01T00:00:00+00:00";

/// Copy migrations 0001 through 0010 so the test can build a v10 database
/// without hand-maintaining a copy of the old schema. The runtime migrator
/// reads the same files the embedded migrator was built from, so the recorded
/// checksums match when `AdminStorage::connect` applies the newer migrations.
fn legacy_migrations(destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let version: i64 = name
            .split('_')
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(i64::MAX);
        if version <= 10 {
            std::fs::copy(entry.path(), destination.join(&name)).unwrap();
        }
    }
}

async fn populated_v10(path: &Path) {
    let migrations = path.with_extension("migrations");
    legacy_migrations(&migrations);
    let migrator = sqlx::migrate::Migrator::new(migrations.as_path())
        .await
        .unwrap();

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();

    let statements = [
        format!(
            "INSERT INTO organizations (id, name, created_at) \
             VALUES ('org_legacy', 'Legacy', '{CREATED_AT}')"
        ),
        format!(
            "INSERT INTO users (id, org_id, name, role, token_hash, created_at, updated_at, \
             token_issued_at, token_expires_at) \
             VALUES ('user_legacy', 'org_legacy', 'Legacy User', 'owner', 'legacy-token-hash', \
             '{CREATED_AT}', '{CREATED_AT}', '{CREATED_AT}', NULL)"
        ),
        format!(
            "INSERT INTO machines (id, name, hostname, os, arch, daemon_version, \
             protocol_version, status, public_key, created_at, updated_at) \
             VALUES ('m_legacy', 'legacy', 'legacy.local', 'linux', 'x86_64', '0.1.0', 1, \
             'online', 'legacy-key', '{CREATED_AT}', '{CREATED_AT}')"
        ),
        format!(
            "INSERT INTO machine_orgs (machine_id, org_id, created_at) \
             VALUES ('m_legacy', 'org_legacy', '{CREATED_AT}')"
        ),
        format!(
            "INSERT INTO workspaces (id, machine_id, name, root_path, is_git_repository, \
             created_at, updated_at) \
             VALUES ('ws_legacy', 'm_legacy', 'legacy', '/tmp/legacy', 1, \
             '{CREATED_AT}', '{CREATED_AT}')"
        ),
        format!(
            "INSERT INTO agent_sessions (id, machine_id, workspace_id, provider, agent_type, \
             status, cwd, metadata_json, created_at, updated_at) \
             VALUES ('s_legacy', 'm_legacy', 'ws_legacy', 'mock', 'mock', 'completed', \
             '/tmp/legacy', '{{}}', '{CREATED_AT}', '{CREATED_AT}')"
        ),
        format!(
            "INSERT INTO pairing_codes (id, code_hash, org_id, created_by, expires_at, \
             used_at, used_by_machine_id, created_at) \
             VALUES ('pair_used', 'used-hash', 'org_legacy', 'admin', '{EXPIRES_AT}', \
             '{CREATED_AT}', 'm_legacy', '{CREATED_AT}')"
        ),
        format!(
            "INSERT INTO pairing_codes (id, code_hash, org_id, created_by, expires_at, created_at) \
             VALUES ('pair_pending', 'pending-hash', 'org_legacy', 'admin', '{EXPIRES_AT}', \
             '{CREATED_AT}')"
        ),
        format!(
            "INSERT INTO audit_logs (actor, org_id, action, subject, created_at) \
             VALUES ('admin', 'org_legacy', 'machine.paired', 'm_legacy', '{CREATED_AT}')"
        ),
    ];
    for statement in statements {
        sqlx::query(&statement).execute(&pool).await.unwrap();
    }
    pool.close().await;
}

#[tokio::test]
async fn upgrading_a_populated_v10_database_keeps_history_and_mints_memberships() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admin.db");
    populated_v10(&path).await;

    let storage = AdminStorage::connect(&path).await.unwrap();
    assert!(storage.pending_migrations().await.unwrap().is_empty());

    // The legacy account keeps its bearer token but gains no email address.
    let user = storage.users.get("user_legacy").await.unwrap().unwrap();
    assert_eq!(user.email, None);
    assert_eq!(user.name, "Legacy User");
    assert_eq!(
        user.token_issued_at.map(|value| value.to_rfc3339()),
        Some(CREATED_AT.to_string())
    );
    assert_eq!(user.token_expires_at, None);
    assert_eq!(
        storage
            .users
            .by_token_hash("legacy-token-hash")
            .await
            .unwrap()
            .map(|user| user.id),
        Some("user_legacy".to_string())
    );

    // The legacy org_id/role pair becomes one membership.
    let membership = storage
        .memberships
        .get("org_legacy", "user_legacy")
        .await
        .unwrap()
        .expect("legacy user must keep a membership");
    assert_eq!(membership.role, UserRole::Owner);

    let organization = storage
        .organizations
        .get("org_legacy")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(organization.kind, OrganizationKind::Team);
    assert_eq!(organization.deleted_at, None);
    assert_eq!(organization.updated_at, organization.created_at);

    // Machines, workspaces, agent sessions and audit rows are untouched.
    assert!(storage.machines.get("m_legacy").await.unwrap().is_some());
    assert_eq!(
        storage.machine_orgs.get("m_legacy").await.unwrap(),
        Some("org_legacy".to_string())
    );
    assert!(storage.workspaces.get("ws_legacy").await.unwrap().is_some());
    assert!(storage.sessions.get("s_legacy").await.unwrap().is_some());

    let (pairing_codes,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pairing_codes WHERE org_id = 'org_legacy'")
            .fetch_one(storage.pool())
            .await
            .unwrap();
    assert_eq!(pairing_codes, 2, "used and pending codes both survive");
    let (used_codes,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pairing_codes WHERE used_at IS NOT NULL")
            .fetch_one(storage.pool())
            .await
            .unwrap();
    assert_eq!(used_codes, 1);

    let (audit_rows,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE org_id = 'org_legacy'")
            .fetch_one(storage.pool())
            .await
            .unwrap();
    assert_eq!(audit_rows, 1);

    let violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(storage.pool())
        .await
        .unwrap();
    assert!(violations.is_empty(), "foreign keys: {violations:?}");
}

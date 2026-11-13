//! SQLite-backed persistence shared by the daemon and the control plane.
//!
//! Every method uses the runtime `sqlx` query API so the crate builds without a
//! `DATABASE_URL`. Timestamps are stored as RFC 3339 text and structured values
//! as JSON text.

use std::path::Path;
use std::time::Duration;

use agent_fleet_common::{now, ApiError, Timestamp};
use agent_fleet_protocol::agent::{
    AgentPermissionRequest, AgentSession, PermissionStatus, ProviderInfo, SessionStatus,
};
use agent_fleet_protocol::audit::AuditLogEntry;
use agent_fleet_protocol::authz::{Organization, User, UserRole};
use agent_fleet_protocol::git::GitFileStatus;
use agent_fleet_protocol::machine::{Machine, MachineStatus};
use agent_fleet_protocol::workspace::{EntryType, Workspace, WorkspaceEntry, WorkspaceSummary};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

use crate::Result;

/// Embedded migrations shared by both database roles.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/schema");

/// A cloneable handle to one SQLite connection pool.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Open (or create) a SQLite database at `path` and apply migrations.
    ///
    /// Parent directories are created when missing. The pool uses WAL mode, a
    /// five second busy timeout, foreign keys and at most eight connections.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;

        let database = Self { pool };
        database.migrate().await?;
        Ok(database)
    }

    /// Open a private in-memory database and apply migrations.
    ///
    /// The pool is limited to one connection so every caller observes the same
    /// in-memory database.
    pub async fn connect_in_memory() -> Result<Self> {
        let options: SqliteConnectOptions = "sqlite::memory:".parse()?;
        let options = options
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        let database = Self { pool };
        database.migrate().await?;
        Ok(database)
    }

    /// Apply every embedded migration. Running this twice is safe.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Borrow the underlying pool for advanced callers and tests.
    pub fn pool(&self) -> &sqlx::SqlitePool {
        &self.pool
    }

    /// Insert a machine or replace its mutable fields.
    pub async fn upsert_machine(&self, m: &Machine) -> Result<()> {
        let providers_json = serde_json::to_string(&m.providers)?;
        let workspaces_json = serde_json::to_string(&m.workspaces)?;

        sqlx::query(
            r#"
            INSERT INTO machines (
                id, name, hostname, os, arch, daemon_version, protocol_version,
                status, public_key, connected_at, last_seen_at, providers_json,
                workspaces_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                hostname = excluded.hostname,
                os = excluded.os,
                arch = excluded.arch,
                daemon_version = excluded.daemon_version,
                protocol_version = excluded.protocol_version,
                status = excluded.status,
                public_key = excluded.public_key,
                connected_at = excluded.connected_at,
                last_seen_at = excluded.last_seen_at,
                providers_json = excluded.providers_json,
                workspaces_json = excluded.workspaces_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&m.id)
        .bind(&m.name)
        .bind(&m.hostname)
        .bind(&m.os)
        .bind(&m.arch)
        .bind(&m.daemon_version)
        .bind(i64::from(m.protocol_version))
        .bind(machine_status_text(m.status))
        .bind(&m.public_key)
        .bind(format_opt_ts(m.connected_at))
        .bind(format_opt_ts(m.last_seen_at))
        .bind(providers_json)
        .bind(workspaces_json)
        .bind(format_ts(m.created_at))
        .bind(format_ts(m.updated_at))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch one machine by id.
    pub async fn get_machine(&self, id: &str) -> Result<Option<Machine>> {
        let row = sqlx::query_as::<_, MachineRow>(
            r#"
            SELECT id, name, hostname, os, arch, daemon_version, protocol_version,
                   status, public_key, connected_at, last_seen_at, providers_json,
                   workspaces_json, created_at, updated_at
            FROM machines
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(MachineRow::into_machine))
    }

    /// List every known machine, ordered by name.
    pub async fn list_machines(&self) -> Result<Vec<Machine>> {
        let rows = sqlx::query_as::<_, MachineRow>(
            r#"
            SELECT id, name, hostname, os, arch, daemon_version, protocol_version,
                   status, public_key, connected_at, last_seen_at, providers_json,
                   workspaces_json, created_at, updated_at
            FROM machines
            ORDER BY name ASC, id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(MachineRow::into_machine).collect())
    }

    /// Update liveness fields for a machine.
    ///
    /// `None` timestamp arguments keep the currently stored value.
    pub async fn update_machine_status(
        &self,
        id: &str,
        status: MachineStatus,
        connected_at: Option<Timestamp>,
        last_seen_at: Option<Timestamp>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE machines
            SET status = ?2,
                connected_at = COALESCE(?3, connected_at),
                last_seen_at = COALESCE(?4, last_seen_at),
                updated_at = ?5
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .bind(machine_status_text(status))
        .bind(format_opt_ts(connected_at))
        .bind(format_opt_ts(last_seen_at))
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Delete a machine row.
    pub async fn delete_machine(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM machines WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM machine_orgs WHERE machine_id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Insert a workspace or replace its mutable fields.
    pub async fn upsert_workspace(&self, w: &Workspace) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO workspaces (
                id, machine_id, project_id, name, root_path,
                is_git_repository, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(id) DO UPDATE SET
                machine_id = excluded.machine_id,
                project_id = excluded.project_id,
                name = excluded.name,
                root_path = excluded.root_path,
                is_git_repository = excluded.is_git_repository,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&w.id)
        .bind(&w.machine_id)
        .bind(&w.project_id)
        .bind(&w.name)
        .bind(&w.root_path)
        .bind(bool_to_int(w.is_git_repository))
        .bind(format_ts(w.created_at))
        .bind(format_ts(w.updated_at))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch one workspace by id.
    pub async fn get_workspace(&self, id: &str) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            r#"
            SELECT id, machine_id, project_id, name, root_path,
                   is_git_repository, created_at, updated_at
            FROM workspaces
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(WorkspaceRow::into_workspace))
    }

    /// List workspaces, optionally restricted to one machine, ordered by name.
    pub async fn list_workspaces(&self, machine_id: Option<&str>) -> Result<Vec<Workspace>> {
        let rows = match machine_id {
            Some(machine_id) => {
                sqlx::query_as::<_, WorkspaceRow>(
                    r#"
                    SELECT id, machine_id, project_id, name, root_path,
                           is_git_repository, created_at, updated_at
                    FROM workspaces
                    WHERE machine_id = ?1
                    ORDER BY name ASC, id ASC
                    "#,
                )
                .bind(machine_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, WorkspaceRow>(
                    r#"
                    SELECT id, machine_id, project_id, name, root_path,
                           is_git_repository, created_at, updated_at
                    FROM workspaces
                    ORDER BY name ASC, id ASC
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows.into_iter().map(WorkspaceRow::into_workspace).collect())
    }

    /// Delete a workspace row.
    pub async fn delete_workspace(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM workspaces WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Insert a session or replace its mutable fields.
    pub async fn upsert_session(&self, s: &AgentSession) -> Result<()> {
        let last_error_json = match &s.last_error {
            Some(error) => Some(serde_json::to_string(error)?),
            None => None,
        };
        let metadata_json = serde_json::to_string(&s.metadata)?;

        sqlx::query(
            r#"
            INSERT INTO agent_sessions (
                id, machine_id, workspace_id, provider, agent_type, status,
                cwd, title, last_error_json, metadata_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(id) DO UPDATE SET
                machine_id = excluded.machine_id,
                workspace_id = excluded.workspace_id,
                provider = excluded.provider,
                agent_type = excluded.agent_type,
                status = excluded.status,
                cwd = excluded.cwd,
                title = excluded.title,
                last_error_json = excluded.last_error_json,
                metadata_json = excluded.metadata_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&s.id)
        .bind(&s.machine_id)
        .bind(&s.workspace_id)
        .bind(&s.provider)
        .bind(&s.agent_type)
        .bind(enum_text(&s.status))
        .bind(&s.cwd)
        .bind(&s.title)
        .bind(last_error_json)
        .bind(metadata_json)
        .bind(format_ts(s.created_at))
        .bind(format_ts(s.updated_at))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch one session by id.
    pub async fn get_session(&self, id: &str) -> Result<Option<AgentSession>> {
        let row = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, machine_id, workspace_id, provider, agent_type, status,
                   cwd, title, last_error_json, metadata_json, created_at, updated_at
            FROM agent_sessions
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(SessionRow::into_session))
    }

    /// List sessions, optionally restricted to one machine, newest first.
    pub async fn list_sessions(&self, machine_id: Option<&str>) -> Result<Vec<AgentSession>> {
        let rows = match machine_id {
            Some(machine_id) => {
                sqlx::query_as::<_, SessionRow>(
                    r#"
                    SELECT id, machine_id, workspace_id, provider, agent_type, status,
                           cwd, title, last_error_json, metadata_json, created_at, updated_at
                    FROM agent_sessions
                    WHERE machine_id = ?1
                    ORDER BY created_at DESC, id DESC
                    "#,
                )
                .bind(machine_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, SessionRow>(
                    r#"
                    SELECT id, machine_id, workspace_id, provider, agent_type, status,
                           cwd, title, last_error_json, metadata_json, created_at, updated_at
                    FROM agent_sessions
                    ORDER BY created_at DESC, id DESC
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows.into_iter().map(SessionRow::into_session).collect())
    }

    /// Delete a session row.
    pub async fn delete_session(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM agent_sessions WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Insert a permission request or replace its mutable fields.
    pub async fn upsert_permission(&self, p: &AgentPermissionRequest) -> Result<()> {
        let input_json = serde_json::to_string(&p.input)?;

        sqlx::query(
            r#"
            INSERT INTO permission_requests (
                id, session_id, turn_id, tool_call_id, tool_name, description,
                input_json, status, created_at, resolved_at, expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(id) DO UPDATE SET
                session_id = excluded.session_id,
                turn_id = excluded.turn_id,
                tool_call_id = excluded.tool_call_id,
                tool_name = excluded.tool_name,
                description = excluded.description,
                input_json = excluded.input_json,
                status = excluded.status,
                resolved_at = excluded.resolved_at,
                expires_at = excluded.expires_at
            "#,
        )
        .bind(&p.id)
        .bind(&p.session_id)
        .bind(&p.turn_id)
        .bind(&p.tool_call_id)
        .bind(&p.tool_name)
        .bind(&p.description)
        .bind(input_json)
        .bind(enum_text(&p.status))
        .bind(format_ts(p.created_at))
        .bind(format_opt_ts(p.resolved_at))
        .bind(format_opt_ts(p.expires_at))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch one permission request by id.
    pub async fn get_permission(&self, id: &str) -> Result<Option<AgentPermissionRequest>> {
        let row = sqlx::query_as::<_, PermissionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, tool_name, description,
                   input_json, status, created_at, resolved_at, expires_at
            FROM permission_requests
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(PermissionRow::into_permission))
    }

    /// List every permission request for a session, newest first.
    pub async fn list_permissions(&self, session_id: &str) -> Result<Vec<AgentPermissionRequest>> {
        let rows = sqlx::query_as::<_, PermissionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, tool_name, description,
                   input_json, status, created_at, resolved_at, expires_at
            FROM permission_requests
            WHERE session_id = ?1
            ORDER BY created_at DESC, id DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(PermissionRow::into_permission)
            .collect())
    }

    /// List only pending permission requests for a session, oldest first.
    pub async fn list_pending_permissions(
        &self,
        session_id: &str,
    ) -> Result<Vec<AgentPermissionRequest>> {
        let rows = sqlx::query_as::<_, PermissionRow>(
            r#"
            SELECT id, session_id, turn_id, tool_call_id, tool_name, description,
                   input_json, status, created_at, resolved_at, expires_at
            FROM permission_requests
            WHERE session_id = ?1 AND status = ?2
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .bind(enum_text(&PermissionStatus::Pending))
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(PermissionRow::into_permission)
            .collect())
    }

    /// Replace all cached entries for one directory in a workspace.
    ///
    /// The delete and inserts run in a single transaction so readers never see
    /// a half-replaced directory.
    pub async fn replace_entries(
        &self,
        workspace_id: &str,
        parent_path: &str,
        entries: &[WorkspaceEntry],
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query("DELETE FROM workspace_entries WHERE workspace_id = ?1 AND parent_path = ?2")
            .bind(workspace_id)
            .bind(parent_path)
            .execute(&mut *transaction)
            .await?;

        for entry in entries {
            sqlx::query(
                r#"
                INSERT INTO workspace_entries (
                    workspace_id, parent_path, path, name, entry_type,
                    size, mtime_ns, git_status, ignored
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(workspace_id, path) DO UPDATE SET
                    parent_path = excluded.parent_path,
                    name = excluded.name,
                    entry_type = excluded.entry_type,
                    size = excluded.size,
                    mtime_ns = excluded.mtime_ns,
                    git_status = excluded.git_status,
                    ignored = excluded.ignored
                "#,
            )
            .bind(workspace_id)
            .bind(parent_path)
            .bind(&entry.path)
            .bind(&entry.name)
            .bind(enum_text(&entry.entry_type))
            .bind(entry.size.and_then(|size| i64::try_from(size).ok()))
            .bind(entry.modified_at.map(ts_to_nanos))
            .bind(entry.git_status.as_ref().map(enum_text))
            .bind(bool_to_int(entry.ignored))
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    /// List cached entries for one directory, ordered by path.
    pub async fn list_entries(
        &self,
        workspace_id: &str,
        parent_path: &str,
    ) -> Result<Vec<WorkspaceEntry>> {
        let rows = sqlx::query_as::<_, EntryRow>(
            r#"
            SELECT path, name, entry_type, size, mtime_ns, git_status, ignored
            FROM workspace_entries
            WHERE workspace_id = ?1 AND parent_path = ?2
            ORDER BY path ASC
            "#,
        )
        .bind(workspace_id)
        .bind(parent_path)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(EntryRow::into_entry).collect())
    }

    /// Delete every cached entry at or below `path_prefix` in a workspace.
    ///
    /// A row matches when its path equals the prefix or starts with
    /// `prefix + '/'`. LIKE wildcards inside the prefix are escaped.
    pub async fn delete_entries_under(&self, workspace_id: &str, path_prefix: &str) -> Result<()> {
        let pattern = format!("{}/%", escape_like(path_prefix));

        sqlx::query(
            r#"
            DELETE FROM workspace_entries
            WHERE workspace_id = ?1
              AND (path = ?2 OR path LIKE ?3 ESCAPE '\')
            "#,
        )
        .bind(workspace_id)
        .bind(path_prefix)
        .bind(pattern)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Append one audit log record.
    pub async fn append_audit_log(
        &self,
        actor: &str,
        action: &str,
        subject: &str,
        details: serde_json::Value,
    ) -> Result<()> {
        let details_json = serde_json::to_string(&details)?;

        sqlx::query(
            r#"
            INSERT INTO audit_logs (actor, action, subject, details_json, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(actor)
        .bind(action)
        .bind(subject)
        .bind(details_json)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// List audit records newest first, clamped to 1–500 entries.
    pub async fn list_audit_logs(&self, limit: usize) -> Result<Vec<AuditLogEntry>> {
        let limit = limit.clamp(1, 500) as i64;
        let rows: Vec<AuditRow> = sqlx::query_as(
            r#"
            SELECT id, actor, action, subject, details_json, created_at
            FROM audit_logs
            ORDER BY id DESC
            LIMIT ?1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| AuditLogEntry {
                id: row.id,
                actor: row.actor,
                action: row.action,
                subject: row.subject,
                details: serde_json::from_str(&row.details_json).unwrap_or(Value::Null),
                created_at: DateTime::parse_from_rfc3339(&row.created_at)
                    .map(|value| value.with_timezone(&Utc))
                    .unwrap_or_else(|_| now()),
            })
            .collect())
    }

    // -----------------------------------------------------------------------
    // Organizations, users and machine ownership
    // -----------------------------------------------------------------------

    /// Create an organization with a generated id.
    pub async fn create_organization(&self, name: &str) -> Result<Organization> {
        let organization = Organization {
            id: agent_fleet_common::new_id("org"),
            name: name.to_string(),
            created_at: now(),
        };
        sqlx::query("INSERT INTO organizations (id, name, created_at) VALUES (?1, ?2, ?3)")
            .bind(&organization.id)
            .bind(&organization.name)
            .bind(format_ts(organization.created_at))
            .execute(&self.pool)
            .await?;
        Ok(organization)
    }

    /// Insert the default organization when it does not exist yet.
    pub async fn ensure_default_organization(&self) -> Result<Organization> {
        let id = agent_fleet_protocol::authz::DEFAULT_ORG_ID;
        if let Some(organization) = self.get_organization(id).await? {
            return Ok(organization);
        }
        let organization = Organization {
            id: id.to_string(),
            name: "Default".to_string(),
            created_at: now(),
        };
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO organizations (id, name, created_at)
            VALUES (?1, ?2, ?3)
            "#,
        )
        .bind(&organization.id)
        .bind(&organization.name)
        .bind(format_ts(organization.created_at))
        .execute(&self.pool)
        .await?;
        self.get_organization(id).await?.ok_or_else(|| {
            crate::StorageError::InvalidData(
                "failed to read the default organization after inserting it".to_string(),
            )
        })
    }

    /// Fetch one organization.
    pub async fn get_organization(&self, id: &str) -> Result<Option<Organization>> {
        let row: Option<OrganizationRow> =
            sqlx::query_as("SELECT id, name, created_at FROM organizations WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(Organization::from))
    }

    /// List organizations oldest first.
    pub async fn list_organizations(&self) -> Result<Vec<Organization>> {
        let rows: Vec<OrganizationRow> =
            sqlx::query_as("SELECT id, name, created_at FROM organizations ORDER BY created_at")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(Organization::from).collect())
    }

    /// Insert a user. `token_hash` must already be a one-way hash.
    pub async fn create_user(
        &self,
        org_id: &str,
        name: &str,
        role: UserRole,
        token_hash: &str,
    ) -> Result<User> {
        let timestamp = now();
        let user = User {
            id: agent_fleet_common::new_id("user"),
            org_id: org_id.to_string(),
            name: name.to_string(),
            role,
            created_at: timestamp,
            updated_at: timestamp,
        };
        sqlx::query(
            r#"
            INSERT INTO users (id, org_id, name, role, token_hash, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&user.id)
        .bind(&user.org_id)
        .bind(&user.name)
        .bind(user.role.as_str())
        .bind(token_hash)
        .bind(format_ts(timestamp))
        .bind(format_ts(timestamp))
        .execute(&self.pool)
        .await?;
        Ok(user)
    }

    /// List users, optionally restricted to one organization.
    pub async fn list_users(&self, org_id: Option<&str>) -> Result<Vec<User>> {
        let rows: Vec<UserRow> = match org_id {
            Some(org_id) => {
                sqlx::query_as(
                    "SELECT id, org_id, name, role, created_at, updated_at FROM users                      WHERE org_id = ?1 ORDER BY created_at",
                )
                .bind(org_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT id, org_id, name, role, created_at, updated_at FROM users                      ORDER BY created_at",
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows
            .into_iter()
            .filter_map(|row| User::try_from(row).ok())
            .collect())
    }

    /// Fetch one user.
    pub async fn get_user(&self, id: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT id, org_id, name, role, created_at, updated_at FROM users WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| User::try_from(row).ok()))
    }

    /// Resolve a user by the hash of their bearer token.
    pub async fn user_by_token_hash(&self, token_hash: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT id, org_id, name, role, created_at, updated_at FROM users              WHERE token_hash = ?1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| User::try_from(row).ok()))
    }

    /// Delete a user. Returns false when it did not exist.
    pub async fn delete_user(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Replace a user's token hash. Returns false when the user does not exist.
    pub async fn update_user_token_hash(&self, id: &str, token_hash: &str) -> Result<bool> {
        let result = sqlx::query("UPDATE users SET token_hash = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(token_hash)
            .bind(format_ts(now()))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Assign (or reassign) a machine to an organization.
    pub async fn assign_machine_org(&self, machine_id: &str, org_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO machine_orgs (machine_id, org_id, created_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(machine_id) DO UPDATE SET org_id = excluded.org_id
            "#,
        )
        .bind(machine_id)
        .bind(org_id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Assign a machine to an organization only when it has no owner yet.
    pub async fn ensure_machine_org(&self, machine_id: &str, org_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO machine_orgs (machine_id, org_id, created_at)
            VALUES (?1, ?2, ?3)
            "#,
        )
        .bind(machine_id)
        .bind(org_id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Organization that owns a machine, if any.
    pub async fn machine_org(&self, machine_id: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT org_id FROM machine_orgs WHERE machine_id = ?1")
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|row| row.0))
    }

    /// Machine ids owned by one organization.
    pub async fn machine_ids_for_org(&self, org_id: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT machine_id FROM machine_orgs WHERE org_id = ?1")
                .bind(org_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
}

#[derive(Debug, FromRow)]
struct OrganizationRow {
    id: String,
    name: String,
    created_at: String,
}

impl From<OrganizationRow> for Organization {
    fn from(row: OrganizationRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            created_at: parse_ts(&row.created_at),
        }
    }
}

#[derive(Debug, FromRow)]
struct UserRow {
    id: String,
    org_id: String,
    name: String,
    role: String,
    created_at: String,
    updated_at: String,
}

impl TryFrom<UserRow> for User {
    type Error = ();

    fn try_from(row: UserRow) -> std::result::Result<Self, Self::Error> {
        let role = UserRole::parse(&row.role).ok_or(())?;
        Ok(Self {
            id: row.id,
            org_id: row.org_id,
            name: row.name,
            role,
            created_at: parse_ts(&row.created_at),
            updated_at: parse_ts(&row.updated_at),
        })
    }
}

#[derive(Debug, FromRow)]
struct AuditRow {
    id: i64,
    actor: String,
    action: String,
    subject: String,
    details_json: String,
    created_at: String,
}

#[derive(Debug, FromRow)]
struct MachineRow {
    id: String,
    name: String,
    hostname: String,
    os: String,
    arch: String,
    daemon_version: String,
    protocol_version: i64,
    status: String,
    public_key: Option<String>,
    connected_at: Option<String>,
    last_seen_at: Option<String>,
    providers_json: String,
    workspaces_json: String,
    created_at: String,
    updated_at: String,
}

impl MachineRow {
    fn into_machine(self) -> Machine {
        let providers = decode_json_or_default::<Vec<ProviderInfo>>(&self.providers_json);
        let workspaces = decode_json_or_default::<Vec<WorkspaceSummary>>(&self.workspaces_json);

        Machine {
            id: self.id,
            name: self.name,
            hostname: self.hostname,
            os: self.os,
            arch: self.arch,
            daemon_version: self.daemon_version,
            protocol_version: u16::try_from(self.protocol_version).unwrap_or_default(),
            status: parse_machine_status(&self.status),
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
            connected_at: parse_opt_ts(self.connected_at),
            last_seen_at: parse_opt_ts(self.last_seen_at),
            workspace_count: u32::try_from(workspaces.len()).unwrap_or(u32::MAX),
            providers,
            workspaces,
            public_key: self.public_key,
        }
    }
}

#[derive(Debug, FromRow)]
struct WorkspaceRow {
    id: String,
    machine_id: String,
    project_id: Option<String>,
    name: String,
    root_path: String,
    is_git_repository: i64,
    created_at: String,
    updated_at: String,
}

impl WorkspaceRow {
    fn into_workspace(self) -> Workspace {
        Workspace {
            id: self.id,
            machine_id: self.machine_id,
            project_id: self.project_id,
            name: self.name,
            root_path: self.root_path,
            is_git_repository: self.is_git_repository != 0,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
        }
    }
}

#[derive(Debug, FromRow)]
struct SessionRow {
    id: String,
    machine_id: String,
    workspace_id: Option<String>,
    provider: String,
    agent_type: String,
    status: String,
    cwd: String,
    title: Option<String>,
    last_error_json: Option<String>,
    metadata_json: String,
    created_at: String,
    updated_at: String,
}

impl SessionRow {
    fn into_session(self) -> AgentSession {
        AgentSession {
            id: self.id,
            machine_id: self.machine_id,
            workspace_id: self.workspace_id,
            provider: self.provider,
            agent_type: self.agent_type,
            status: parse_session_status(&self.status),
            cwd: self.cwd,
            title: self.title,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
            last_error: decode_opt_json::<ApiError>(self.last_error_json),
            metadata: decode_json_value(&self.metadata_json),
        }
    }
}

#[derive(Debug, FromRow)]
struct PermissionRow {
    id: String,
    session_id: String,
    turn_id: Option<String>,
    tool_call_id: Option<String>,
    tool_name: String,
    description: String,
    input_json: String,
    status: String,
    created_at: String,
    resolved_at: Option<String>,
    expires_at: Option<String>,
}

impl PermissionRow {
    fn into_permission(self) -> AgentPermissionRequest {
        AgentPermissionRequest {
            id: self.id,
            session_id: self.session_id,
            turn_id: self.turn_id,
            tool_call_id: self.tool_call_id,
            tool_name: self.tool_name,
            description: self.description,
            input: decode_json_value(&self.input_json),
            status: parse_permission_status(&self.status),
            created_at: parse_ts(&self.created_at),
            resolved_at: parse_opt_ts(self.resolved_at),
            expires_at: parse_opt_ts(self.expires_at),
        }
    }
}

#[derive(Debug, FromRow)]
struct EntryRow {
    path: String,
    name: String,
    entry_type: String,
    size: Option<i64>,
    mtime_ns: Option<i64>,
    git_status: Option<String>,
    ignored: i64,
}

impl EntryRow {
    fn into_entry(self) -> WorkspaceEntry {
        WorkspaceEntry {
            path: self.path,
            name: self.name,
            entry_type: enum_from_text(&self.entry_type).unwrap_or(EntryType::File),
            size: self.size.and_then(|size| u64::try_from(size).ok()),
            modified_at: self.mtime_ns.map(nanos_to_ts),
            git_status: self
                .git_status
                .and_then(|status| enum_from_text::<GitFileStatus>(&status)),
            ignored: self.ignored != 0,
        }
    }
}

fn bool_to_int(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn format_ts(value: Timestamp) -> String {
    value.to_rfc3339()
}

fn format_opt_ts(value: Option<Timestamp>) -> Option<String> {
    value.map(format_ts)
}

fn parse_ts(value: &str) -> Timestamp {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .unwrap_or_default()
}

fn parse_opt_ts(value: Option<String>) -> Option<Timestamp> {
    value.as_deref().map(parse_ts)
}

fn ts_to_nanos(value: Timestamp) -> i64 {
    value
        .timestamp_nanos_opt()
        .unwrap_or_else(|| value.timestamp_millis().saturating_mul(1_000_000))
}

fn nanos_to_ts(value: i64) -> Timestamp {
    DateTime::<Utc>::from_timestamp_nanos(value)
}

fn decode_json_or_default<T: DeserializeOwned + Default>(raw: &str) -> T {
    serde_json::from_str(raw).unwrap_or_default()
}

fn decode_json_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

fn decode_opt_json<T: DeserializeOwned>(raw: Option<String>) -> Option<T> {
    raw.and_then(|value| serde_json::from_str(&value).ok())
}

fn enum_text<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn enum_from_text<T: DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(Value::String(text.to_owned())).ok()
}

fn machine_status_text(status: MachineStatus) -> &'static str {
    match status {
        MachineStatus::Online => "online",
        MachineStatus::Offline => "offline",
        MachineStatus::Degraded => "degraded",
        MachineStatus::Unknown => "unknown",
    }
}

fn parse_machine_status(text: &str) -> MachineStatus {
    match text {
        "online" => MachineStatus::Online,
        "offline" => MachineStatus::Offline,
        "degraded" => MachineStatus::Degraded,
        _ => MachineStatus::Unknown,
    }
}

fn parse_session_status(text: &str) -> SessionStatus {
    enum_from_text(text).unwrap_or(SessionStatus::Failed)
}

fn parse_permission_status(text: &str) -> PermissionStatus {
    enum_from_text(text).unwrap_or(PermissionStatus::Cancelled)
}

/// Escape `%`, `_` and `\` so a user path can be embedded in a LIKE pattern.
fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

//! Daemon-side storage.
//!
//! This crate owns the daemon database, its migrations and every repository it
//! uses: the authoritative transcript (turns, messages, tool calls), workspace
//! metadata, the directory index, sessions and permissions. It shares only
//! protocol domain models with `admin-storage`, never tables or storage code.

pub mod event_store;
pub mod idempotency;
pub mod messages;
pub mod outbox;
pub mod permissions;
pub mod questions;
pub mod sessions;
pub mod tool_calls;
pub mod turns;
pub mod workspace_entries;
pub mod workspaces;

mod error;
mod helpers;
mod rows;

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

pub use error::{Result, StorageError};
pub use outbox::{EventOutbox, OutboxEntry, ADMIN_CONSUMER};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Storage used by the machine daemon.
#[derive(Clone)]
pub struct DaemonStorage {
    /// Workspace metadata.
    pub workspaces: workspaces::Workspaces,
    /// Agent sessions.
    pub sessions: sessions::Sessions,
    /// Permission requests and decisions.
    pub permissions: permissions::Permissions,
    /// User questions and decisions.
    pub questions: questions::Questions,
    /// Cached workspace directory entries.
    pub entries: workspace_entries::WorkspaceEntries,
    /// Turn lifecycle records.
    pub turns: turns::Turns,
    /// Conversation messages.
    pub messages: messages::Messages,
    /// Structured tool calls.
    pub tool_calls: tool_calls::ToolCalls,
    /// Durable event outbox and admin cursor.
    pub outbox: outbox::EventOutbox,
    /// Cached responses for retried mutation requests.
    pub idempotency: idempotency::IdempotencyKeys,
    pool: SqlitePool,
}

impl DaemonStorage {
    /// Open a SQLite database and apply daemon migrations.
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
        let storage = Self::from_pool(pool, Some(path)).await?;
        restrict_database_permissions(path)?;
        Ok(storage)
    }

    /// Open an in-memory database and apply daemon migrations.
    pub async fn connect_in_memory() -> Result<Self> {
        let options: SqliteConnectOptions = "sqlite::memory:".parse()?;
        let options = options
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Self::from_pool(pool, None).await
    }

    async fn from_pool(pool: SqlitePool, path: Option<&Path>) -> Result<Self> {
        prepare_schema(&pool, path, ROLE).await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self {
            workspaces: workspaces::Workspaces::new(pool.clone()),
            sessions: sessions::Sessions::new(pool.clone()),
            permissions: permissions::Permissions::new(pool.clone()),
            questions: questions::Questions::new(pool.clone()),
            entries: workspace_entries::WorkspaceEntries::new(pool.clone()),
            turns: turns::Turns::new(pool.clone()),
            messages: messages::Messages::new(pool.clone()),
            tool_calls: tool_calls::ToolCalls::new(pool.clone()),
            outbox: outbox::EventOutbox::new(pool.clone()),
            idempotency: idempotency::IdempotencyKeys::new(pool.clone()),
            pool,
        })
    }

    /// Apply embedded daemon migrations.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Borrow the underlying pool for advanced callers and tests.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// This database's role marker in `storage_meta`.
pub(crate) const ROLE: &str = "daemon";

/// Restrict the SQLite database (and its WAL companions) to its owner.
#[cfg(unix)]
fn restrict_database_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let mut shm = path.as_os_str().to_os_string();
    shm.push("-shm");
    for candidate in [
        path.to_path_buf(),
        std::path::PathBuf::from(wal),
        std::path::PathBuf::from(shm),
    ] {
        if candidate.exists() {
            let mut permissions = std::fs::metadata(&candidate)?.permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(&candidate, permissions)?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_database_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

/// Verify that this database belongs to the daemon role before touching it.
///
/// See the admin storage equivalent: a role mismatch is rejected without
/// modification, and a legacy shared-schema database is backed up first.
async fn prepare_schema(pool: &SqlitePool, path: Option<&Path>, role: &str) -> Result<()> {
    let has_meta: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'storage_meta'",
    )
    .fetch_optional(pool)
    .await?;
    if has_meta.is_some() {
        let stored: Option<(String,)> =
            sqlx::query_as("SELECT value FROM storage_meta WHERE key = 'role'")
                .fetch_optional(pool)
                .await?;
        match stored {
            Some((value,)) if value == role => return Ok(()),
            Some((value,)) => {
                return Err(StorageError::InvalidData(format!(
                    "database role is '{value}', but this process requires '{role}'"
                )))
            }
            None => {
                return Err(StorageError::InvalidData(
                    "database has a storage_meta table without a role marker".to_owned(),
                ))
            }
        }
    }

    let has_tables: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    if has_tables.is_none() {
        return Ok(());
    }

    if let Some(path) = path {
        let backup = path.with_extension(format!(
            "pre-split-{}-{}.bak",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or_default(),
            std::process::id()
        ));
        let backup_str = backup.to_string_lossy().into_owned();
        sqlx::query("VACUUM INTO ?1")
            .bind(backup_str)
            .execute(pool)
            .await?;
    }

    let has_sqlx: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?;
    if has_sqlx.is_some() {
        sqlx::query("DROP TABLE _sqlx_migrations")
            .execute(pool)
            .await?;
    }
    Ok(())
}

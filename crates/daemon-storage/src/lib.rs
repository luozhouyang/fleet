//! Daemon-side storage.
//!
//! This crate owns the daemon database, its migrations and every repository it
//! uses: the authoritative transcript (turns, messages, tool calls), workspace
//! metadata, the directory index, sessions and permissions. It shares only
//! protocol domain models with `admin-storage`, never tables or storage code.

pub mod event_store;
pub mod messages;
pub mod outbox;
pub mod permissions;
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
        Self::from_pool(pool).await
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
        Self::from_pool(pool).await
    }

    async fn from_pool(pool: SqlitePool) -> Result<Self> {
        adopt_legacy_shared_schema(&pool).await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self {
            workspaces: workspaces::Workspaces::new(pool.clone()),
            sessions: sessions::Sessions::new(pool.clone()),
            permissions: permissions::Permissions::new(pool.clone()),
            entries: workspace_entries::WorkspaceEntries::new(pool.clone()),
            turns: turns::Turns::new(pool.clone()),
            messages: messages::Messages::new(pool.clone()),
            tool_calls: tool_calls::ToolCalls::new(pool.clone()),
            outbox: outbox::EventOutbox::new(pool.clone()),
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

async fn adopt_legacy_shared_schema(pool: &SqlitePool) -> Result<()> {
    let has_meta: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'storage_meta'",
    )
    .fetch_optional(pool)
    .await?;
    if has_meta.is_some() {
        return Ok(());
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

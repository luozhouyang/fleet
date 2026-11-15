//! Admin control-plane storage.
//!
//! This crate owns the admin database, its migrations and every repository it
//! uses: machines, workspace/session metadata, permission routing, audit,
//! organizations, users and machine ownership. It shares only protocol domain
//! models with `daemon-storage`, never tables or storage code.

pub mod audit;
pub mod machine_orgs;
pub mod machines;
pub mod organizations;
pub mod permissions;
pub mod sessions;
pub mod users;
pub mod workspaces;

mod error;
mod helpers;
mod rows;

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

pub use error::{Result, StorageError};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Storage used by the agent admin.
#[derive(Clone)]
pub struct AdminStorage {
    /// Machines and revocations.
    pub machines: machines::Machines,
    /// Workspace metadata mirrored from daemons.
    pub workspaces: workspaces::Workspaces,
    /// Session metadata.
    pub sessions: sessions::Sessions,
    /// Permission routing state.
    pub permissions: permissions::Permissions,
    /// Audit records.
    pub audit: audit::Audit,
    /// Organizations.
    pub organizations: organizations::Organizations,
    /// Users and token hashes.
    pub users: users::Users,
    /// Machine-to-organization ownership.
    pub machine_orgs: machine_orgs::MachineOrgs,
    pool: SqlitePool,
}

impl AdminStorage {
    /// Open a SQLite database and apply admin migrations.
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

    /// Open an in-memory database and apply admin migrations.
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
            machines: machines::Machines::new(pool.clone()),
            workspaces: workspaces::Workspaces::new(pool.clone()),
            sessions: sessions::Sessions::new(pool.clone()),
            permissions: permissions::Permissions::new(pool.clone()),
            audit: audit::Audit::new(pool.clone()),
            organizations: organizations::Organizations::new(pool.clone()),
            users: users::Users::new(pool.clone()),
            machine_orgs: machine_orgs::MachineOrgs::new(pool.clone()),
            pool,
        })
    }

    /// Apply embedded admin migrations.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Borrow the underlying pool for advanced callers and tests.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// The pre-split storage crate used one migration history for both roles.
/// When such a database is opened, discard that history so the role-specific
/// migrations can run against the already existing tables.
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

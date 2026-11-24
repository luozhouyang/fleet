//! Admin control-plane storage.
//!
//! This crate owns the admin database, its migrations and every repository it
//! uses: machines, workspace/session metadata, audit, organizations, users,
//! machine ownership and durable event deduplication. It shares only protocol
//! domain models with `daemon-storage`, never tables or storage code.

pub mod audit;
pub mod auth_identities;
pub mod auth_sessions;
pub mod invitations;
pub mod machine_orgs;
pub mod machines;
pub mod memberships;
pub mod oauth_states;
pub mod organizations;
pub mod pairing_codes;
pub mod processed_events;
pub mod realtime_tickets;
pub mod sessions;
pub mod transaction;
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
    /// Audit records.
    pub audit: audit::Audit,
    /// Organizations.
    pub organizations: organizations::Organizations,
    /// Users and token hashes.
    pub users: users::Users,
    /// Organization memberships.
    pub memberships: memberships::Memberships,
    /// Organization invitations.
    pub invitations: invitations::Invitations,
    /// Linked GitHub/Google identities.
    pub auth_identities: auth_identities::AuthIdentities,
    /// Browser sessions.
    pub auth_sessions: auth_sessions::AuthSessions,
    /// Single-use OAuth authorization state.
    pub oauth_states: oauth_states::OAuthStates,
    /// Single-use WebSocket tickets.
    pub realtime_tickets: realtime_tickets::RealtimeTickets,
    /// Machine-to-organization ownership.
    pub machine_orgs: machine_orgs::MachineOrgs,
    /// Pairing codes.
    pub pairing_codes: pairing_codes::PairingCodes,
    /// Durable deduplication for applied agent events.
    pub processed_events: processed_events::ProcessedEvents,
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
        let storage = Self::from_pool(pool, Some(path)).await?;
        restrict_database_permissions(path)?;
        Ok(storage)
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
        Self::from_pool(pool, None).await
    }

    async fn from_pool(pool: SqlitePool, path: Option<&Path>) -> Result<Self> {
        prepare_schema(&pool, path, ROLE).await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self {
            machines: machines::Machines::new(pool.clone()),
            workspaces: workspaces::Workspaces::new(pool.clone()),
            sessions: sessions::Sessions::new(pool.clone()),
            audit: audit::Audit::new(pool.clone()),
            organizations: organizations::Organizations::new(pool.clone()),
            users: users::Users::new(pool.clone()),
            memberships: memberships::Memberships::new(pool.clone()),
            invitations: invitations::Invitations::new(pool.clone()),
            auth_identities: auth_identities::AuthIdentities::new(pool.clone()),
            auth_sessions: auth_sessions::AuthSessions::new(pool.clone()),
            oauth_states: oauth_states::OAuthStates::new(pool.clone()),
            realtime_tickets: realtime_tickets::RealtimeTickets::new(pool.clone()),
            machine_orgs: machine_orgs::MachineOrgs::new(pool.clone()),
            pairing_codes: pairing_codes::PairingCodes::new(pool.clone()),
            processed_events: processed_events::ProcessedEvents::new(pool.clone()),
            pool,
        })
    }

    /// Apply embedded admin migrations.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Run a trivial query to confirm that SQLite is reachable.
    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Return the embedded migration versions that are not recorded as
    /// successfully applied.
    ///
    /// An empty result means the schema is up to date. A database without a
    /// migration table reports every expected version, because it has never
    /// been migrated.
    pub async fn pending_migrations(&self) -> Result<Vec<i64>> {
        let expected: Vec<i64> = MIGRATOR.iter().map(|migration| migration.version).collect();
        let has_table: Option<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_optional(&self.pool)
        .await?;
        if has_table.is_none() {
            return Ok(expected);
        }

        let applied: Vec<(i64, bool)> =
            sqlx::query_as("SELECT version, success FROM _sqlx_migrations")
                .fetch_all(&self.pool)
                .await?;
        let applied: std::collections::HashSet<i64> = applied
            .into_iter()
            .filter(|(_, success)| *success)
            .map(|(version, _)| version)
            .collect();
        Ok(expected
            .into_iter()
            .filter(|version| !applied.contains(version))
            .collect())
    }

    /// Borrow the underlying pool for advanced callers and tests.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// This database's role marker in `storage_meta`.
pub(crate) const ROLE: &str = "admin";

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

/// Verify that this database belongs to the admin role before touching it.
///
/// A pre-split database has no `storage_meta` table but shares one migration
/// history. Such a database is copied to a timestamped backup before the
/// role-specific migration drops the other role's tables. A database that
/// declares a different role is rejected without modification: pointing the
/// admin at a daemon database (or vice versa) must never destroy data.
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
        // Brand-new database: the role-specific migrations create the schema.
        return Ok(());
    }

    // Pre-split database. Back it up before adopting it.
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

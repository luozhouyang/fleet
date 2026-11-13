//! SQLite persistence for the Agent Fleet daemon and control plane.
//!
//! The crate owns the only SQLite pool in the workspace. Every public type is
//! defined in `agent-fleet-protocol`; this crate only maps those types to the
//! shared schema in `migrations/schema`.
//!
//! ```no_run
//! # async fn example() -> agent_fleet_storage::Result<()> {
//! let database = agent_fleet_storage::Database::connect_in_memory().await?;
//! let machines = database.list_machines().await?;
//! println!("{} machines", machines.len());
//! # Ok(())
//! # }
//! ```

mod database;
mod error;

pub use database::Database;
pub use error::{Result, StorageError};

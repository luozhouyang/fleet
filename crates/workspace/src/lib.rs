//! Workspace filesystem access for the Agent Fleet daemon.
//!
//! The crate provides a confined view of one or more local directories:
//! registration, lazy directory listing with ignore rules and pagination,
//! bounded file reads with ETag caching, text search, and filesystem change
//! notifications.
//!
//! All paths crossing the API are workspace-relative and are validated by
//! [`PathPolicy`]. The filesystem is always authoritative; the SQLite
//! `workspace_entries` table is a best-effort cache.

#![warn(missing_docs)]

mod directory_reader;
mod error;
mod file_reader;
mod file_watcher;
mod ignore_rules;
mod path_policy;
mod registry;
mod search;
mod service;
mod settings;

pub use error::{Result, WorkspaceError};
pub use file_reader::{detect_language, is_binary};
pub use path_policy::PathPolicy;
pub use service::{WorkspaceService, WorkspaceWatcher, WorkspaceWatcherStop};
pub use settings::{WorkspaceSettings, DEFAULT_IGNORES, DEFAULT_MAX_FILE_BYTES};

use agent_fleet_common::Timestamp;
use chrono::{DateTime, Utc};

/// Convert a filesystem modification time into a protocol timestamp.
pub(crate) fn system_time_to_timestamp(time: std::time::SystemTime) -> Timestamp {
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::<Utc>::from_timestamp(duration.as_secs() as i64, duration.subsec_nanos())
        .unwrap_or_else(Utc::now)
}

//! Settings owned by the workspace crate.
//!
//! The daemon converts its `WorkspaceConfig` into [`WorkspaceSettings`] so this
//! crate does not depend on `agent-fleet-config`.

/// Default maximum number of bytes read from a single file (2 MiB).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Standard directories that are hidden unless `show_ignored` is requested.
pub const DEFAULT_IGNORES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    ".next",
    "__pycache__",
    ".venv",
];

/// Runtime configuration for workspace access.
#[derive(Debug, Clone)]
pub struct WorkspaceSettings {
    /// Maximum number of bytes read from a file in one request.
    pub max_file_bytes: u64,
    /// Directory or file names that are ignored by default.
    pub default_ignores: Vec<String>,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            default_ignores: DEFAULT_IGNORES
                .iter()
                .map(|name| name.to_string())
                .collect(),
        }
    }
}

impl WorkspaceSettings {
    /// Build settings from an explicit size limit and ignore list.
    pub fn new(max_file_bytes: u64, default_ignores: Vec<String>) -> Self {
        Self {
            max_file_bytes,
            default_ignores,
        }
    }
}

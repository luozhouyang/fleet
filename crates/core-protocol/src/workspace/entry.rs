use serde::{Deserialize, Serialize};

use crate::common::Timestamp;
use crate::git::GitFileStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    File,
    Directory,
    Symlink,
}

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    /// Workspace-relative path using `/` separators.
    pub path: String,
    pub name: String,
    pub entry_type: EntryType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_status: Option<GitFileStatus>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
}

/// Response for `workspace.tree.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryListing {
    pub path: String,
    pub entries: Vec<WorkspaceEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Request for `workspace.tree.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListDirectoryRequest {
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_ignored: Option<bool>,
}

use serde::{Deserialize, Serialize};

use crate::common::Timestamp;

/// File status as reported by Git.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Conflicted,
    Ignored,
    Unmodified,
}

/// One changed file. Index and worktree states are preserved separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitFileChange {
    /// Workspace-relative path using `/` separators.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: GitFileStatus,
    /// True when the change is in the index (staged).
    #[serde(default)]
    pub staged: bool,
    /// Raw porcelain v2 XY characters, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_status: Option<String>,
}

/// Snapshot of repository status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<GitFileChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    #[serde(default)]
    pub is_clean: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
}

impl GitStatus {
    pub fn clean() -> Self {
        Self {
            branch: None,
            ahead: 0,
            behind: 0,
            files: Vec::new(),
            head_oid: None,
            is_clean: true,
            detached: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitBranch {
    pub name: String,
    pub is_current: bool,
    #[serde(default)]
    pub is_remote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitLogEntry {
    pub id: String,
    pub short_id: String,
    pub message: String,
    pub author: String,
    pub email: String,
    pub timestamp: Timestamp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
}

/// Repository descriptor for a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitRepository {
    /// Workspace-relative path of the repository root (usually `.`).
    pub root: String,
    /// Absolute path of the git directory (supports worktrees where `.git` is a file).
    pub git_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
}

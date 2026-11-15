//! Git write-operation contracts shared by the daemon, agent admin and UI.

use serde::{Deserialize, Serialize};

/// Paths to stage or unstage, relative to the workspace root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPathsRequest {
    /// One or more workspace-relative paths. `.` stages the whole worktree.
    pub paths: Vec<String>,
}

/// Options for creating a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GitCommitOptions {
    /// Commit message. Required and non-empty.
    pub message: String,
    /// Commit only these paths (leaving the rest staged). When omitted the
    /// whole index is committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// Amend the previous commit instead of creating a new one.
    #[serde(default)]
    pub amend: bool,
}

/// Result of a successful commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitResult {
    /// Full hash of the new commit.
    pub commit: String,
    /// Current branch name, or `HEAD` when detached.
    pub branch: String,
    /// First line of the commit message.
    pub summary: String,
}

/// Check out an existing branch or create and switch to a new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GitCheckoutOptions {
    /// Branch name to switch to.
    pub branch: String,
    /// Create the branch (`git checkout -b`) before switching.
    #[serde(default)]
    pub create: bool,
}

/// Result of a successful checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCheckoutResult {
    /// Branch that is now checked out.
    pub branch: String,
}

use serde::{Deserialize, Serialize};

use super::GitFileStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineType {
    Context,
    Added,
    Deleted,
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub line_type: DiffLineType,
    /// Content without the leading `+`, `-` or space marker.
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line_no: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line_no: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default)]
    pub lines: Vec<DiffLine>,
}

/// Full unified diff for one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: GitFileStatus,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub is_binary: bool,
    /// True when the structured hunks were cut at the per-file render limit.
    /// Fetch the raw diff for the complete patch.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitDiff {
    #[serde(default)]
    pub files: Vec<FileDiff>,
    #[serde(default)]
    pub is_binary: bool,
    /// True when at least one file diff was truncated for rendering.
    #[serde(default)]
    pub truncated: bool,
}

/// Raw unified diff output, used as the complete fallback for a truncated
/// structured diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitRawDiff {
    pub patch: String,
    /// True when the daemon's output cap cut the patch.
    #[serde(default)]
    pub truncated: bool,
}

/// Lightweight per-file summary used before fetching full diffs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: GitFileStatus,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub is_binary: bool,
    #[serde(default)]
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitDiffSummary {
    #[serde(default)]
    pub files: Vec<DiffSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
}

/// Options for `git.diff` and `git.diff.summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GitDiffOptions {
    /// Restrict to a single workspace-relative path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Diff the index against HEAD instead of the worktree against the index.
    #[serde(default)]
    pub staged: bool,
    /// Optional commit range, e.g. `main..HEAD` or a single commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Include untracked files as added-file diffs.
    #[serde(default)]
    pub include_untracked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_lines: Option<u32>,
    /// Ignore whitespace changes.
    #[serde(default)]
    pub ignore_whitespace: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitLogRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

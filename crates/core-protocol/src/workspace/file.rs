use serde::{Deserialize, Serialize};

use crate::common::{Timestamp, WorkspaceId};

/// Metadata for a file without its content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMetadata {
    pub path: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Timestamp>,
    pub is_binary: bool,
    pub etag: String,
}

/// File content returned to the browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub encoding: String,
    pub size: u64,
    pub etag: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub is_binary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Timestamp>,
}

/// Request for `workspace.file.read`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadFileRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// Default size of one `workspace.file.range` response.
pub const DEFAULT_FILE_RANGE_BYTES: u64 = 64 * 1024;

/// Request for `workspace.file.range`.
///
/// The daemon caps `length` at `workspace.max_file_bytes`; callers should
/// request the next range from `offset + decoded data length`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadFileRangeRequest {
    pub path: String,
    /// Starting byte offset. Must not exceed the file size.
    #[serde(default)]
    pub offset: u64,
    /// Maximum bytes to return. Defaults to 64 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

/// One bounded byte range of a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRange {
    pub path: String,
    pub offset: u64,
    /// Base64-encoded bytes.
    pub data: String,
    /// Full size of the file, independent of this range.
    pub total_size: u64,
    /// Weak validator derived from size and modification time.
    pub etag: String,
    #[serde(default)]
    pub is_binary: bool,
    /// True when this range reaches the end of the file.
    #[serde(default)]
    pub eof: bool,
}

/// Response when the client already holds the current content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotModified {
    pub path: String,
    pub etag: String,
}

/// Request for `workspace.file.search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_sensitive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    pub line_number: u64,
    pub line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub matches: Vec<SearchMatch>,
    #[serde(default)]
    pub truncated: bool,
}

/// Event types pushed when a workspace changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceEventType {
    #[serde(rename = "workspace.directory.changed")]
    DirectoryChanged,
    #[serde(rename = "workspace.file.created")]
    FileCreated,
    #[serde(rename = "workspace.file.modified")]
    FileModified,
    #[serde(rename = "workspace.file.deleted")]
    FileDeleted,
    #[serde(rename = "workspace.file.renamed")]
    FileRenamed,
}

impl WorkspaceEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectoryChanged => "workspace.directory.changed",
            Self::FileCreated => "workspace.file.created",
            Self::FileModified => "workspace.file.modified",
            Self::FileDeleted => "workspace.file.deleted",
            Self::FileRenamed => "workspace.file.renamed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEvent {
    pub event_id: String,
    pub workspace_id: WorkspaceId,
    #[serde(rename = "type")]
    pub event_type: WorkspaceEventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub timestamp: Timestamp,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
}

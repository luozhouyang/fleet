use serde::{Deserialize, Serialize};

/// Logical stream frames for large payloads (file content, big diffs).
///
/// Chunks are base64-encoded in the JSON control frame for MVP simplicity; the
/// framing is transport-agnostic so it can later move to binary WebSocket frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamFrame {
    Open(StreamOpen),
    Chunk(StreamChunk),
    End(StreamEnd),
    Cancel(StreamCancel),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamOpen {
    pub stream_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamChunk {
    pub stream_id: String,
    #[serde(default)]
    pub sequence: u64,
    /// Base64-encoded chunk bytes.
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamEnd {
    pub stream_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamCancel {
    pub stream_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl StreamFrame {
    pub fn stream_id(&self) -> &str {
        match self {
            Self::Open(f) => &f.stream_id,
            Self::Chunk(f) => &f.stream_id,
            Self::End(f) => &f.stream_id,
            Self::Cancel(f) => &f.stream_id,
        }
    }
}

/// Default chunk size for streamed payloads.
pub const DEFAULT_STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Maximum accepted chunk bytes (base64-decoded).
pub const MAX_STREAM_CHUNK_BYTES: usize = 1024 * 1024;

/// Maximum number of concurrently open streams per connection.
pub const MAX_CONCURRENT_STREAMS: usize = 64;

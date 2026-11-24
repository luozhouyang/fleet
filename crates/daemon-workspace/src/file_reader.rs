//! Bounded file reading, binary detection and ETag support.

use std::io::SeekFrom;
use std::path::Path;

use base64::Engine;
use fleet_core_protocol::workspace::{
    FileContent, FileMetadata, FileRange, ReadFileRangeRequest, ReadFileRequest,
    DEFAULT_FILE_RANGE_BYTES,
};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::path_policy::PathPolicy;
use crate::settings::WorkspaceSettings;
use crate::{Result, WorkspaceError};

/// Number of leading bytes inspected for binary content.
const BINARY_SAMPLE_BYTES: usize = 8192;

/// Read one file, returning `None` when the supplied ETag is current.
///
/// The returned content is capped by `min(settings.max_file_bytes,
/// request.max_bytes)`. When the file is longer, `truncated` is set and the
/// ETag incorporates the full size and mtime so tail changes are detected.
pub(crate) async fn read_file(
    policy: &PathPolicy,
    settings: &WorkspaceSettings,
    request: &ReadFileRequest,
) -> Result<Option<FileContent>> {
    let normalized = PathPolicy::normalize_relative(&request.path)?;
    let resolved = policy.resolve_existing(&request.path).await?;
    let metadata = tokio::fs::metadata(&resolved).await?;

    if !metadata.file_type().is_file() {
        return Err(WorkspaceError::InvalidRequest(format!(
            "path is not a regular file: {}",
            request.path
        )));
    }

    let limit = settings
        .max_file_bytes
        .min(request.max_bytes.unwrap_or(u64::MAX));
    let read_limit = metadata.len().min(limit);

    let file = tokio::fs::File::open(&resolved).await?;
    let mut bytes = Vec::with_capacity(read_limit.min(64 * 1024) as usize);
    file.take(read_limit).read_to_end(&mut bytes).await?;

    let truncated = metadata.len() > bytes.len() as u64;
    // A truncated read must not ignore tail changes: mix the full length and
    // mtime into the ETag as well as the returned bytes.
    let etag = if truncated {
        let mut hasher = Sha256::new();
        hasher.update(metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified() {
            if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                hasher.update(duration.as_nanos().to_le_bytes());
            }
        }
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    } else {
        sha256_hex(&bytes)
    };
    if request.etag.as_deref() == Some(etag.as_str()) {
        return Ok(None);
    }

    let binary = is_binary_truncated(&bytes, truncated);
    let content = if binary {
        String::new()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let language = if binary {
        None
    } else {
        detect_language(&normalized).map(str::to_string)
    };

    Ok(Some(FileContent {
        path: normalized,
        content,
        encoding: if binary { "binary" } else { "utf-8" }.to_string(),
        size: metadata.len(),
        etag,
        truncated,
        is_binary: binary,
        language,
        modified_at: metadata
            .modified()
            .ok()
            .map(crate::system_time_to_timestamp),
    }))
}

/// Stat a file without returning its content.
///
/// The ETag is weak: it hashes the full size and modification time, so a
/// caller can page through a large file without hashing every byte first.
pub(crate) async fn stat_file(policy: &PathPolicy, path: &str) -> Result<FileMetadata> {
    let normalized = PathPolicy::normalize_relative(path)?;
    let resolved = policy.resolve_existing(path).await?;
    let metadata = tokio::fs::metadata(&resolved).await?;
    if !metadata.file_type().is_file() {
        return Err(WorkspaceError::InvalidRequest(format!(
            "path is not a regular file: {path}"
        )));
    }
    let header = read_header(&resolved).await?;
    let is_binary = is_binary_truncated(&header, metadata.len() > header.len() as u64);
    Ok(FileMetadata {
        path: normalized,
        size: metadata.len(),
        modified_at: metadata
            .modified()
            .ok()
            .map(crate::system_time_to_timestamp),
        is_binary,
        etag: weak_etag(&metadata),
    })
}

/// Read one bounded byte range of a file.
///
/// `length` defaults to [`DEFAULT_FILE_RANGE_BYTES`] and is capped by
/// `settings.max_file_bytes`; the response carries `eof` so a short chunk is
/// never mistaken for the end of the file. Binary detection uses the file
/// header, not the requested range, so a mid-file chunk is still classified
/// correctly.
pub(crate) async fn read_file_range(
    policy: &PathPolicy,
    settings: &WorkspaceSettings,
    request: &ReadFileRangeRequest,
) -> Result<FileRange> {
    if request.length == Some(0) {
        return Err(WorkspaceError::InvalidRequest(
            "range length must be greater than zero".into(),
        ));
    }
    let normalized = PathPolicy::normalize_relative(&request.path)?;
    let resolved = policy.resolve_existing(&request.path).await?;
    let metadata = tokio::fs::metadata(&resolved).await?;
    if !metadata.file_type().is_file() {
        return Err(WorkspaceError::InvalidRequest(format!(
            "path is not a regular file: {}",
            request.path
        )));
    }
    let size = metadata.len();
    if request.offset > size {
        return Err(WorkspaceError::InvalidRequest(format!(
            "range starts at {} past the end of {normalized} ({size} bytes)",
            request.offset
        )));
    }

    let limit = request
        .length
        .unwrap_or(DEFAULT_FILE_RANGE_BYTES)
        .min(settings.max_file_bytes.max(1));
    let to_read = (size - request.offset).min(limit);

    let mut file = tokio::fs::File::open(&resolved).await?;
    if request.offset > 0 {
        file.seek(SeekFrom::Start(request.offset)).await?;
    }
    let mut bytes = Vec::with_capacity(to_read.min(64 * 1024) as usize);
    file.take(to_read).read_to_end(&mut bytes).await?;

    let header = read_header(&resolved).await?;
    let is_binary = is_binary_truncated(&header, size > header.len() as u64);

    Ok(FileRange {
        path: normalized,
        offset: request.offset,
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        total_size: size,
        etag: weak_etag(&metadata),
        is_binary,
        eof: request.offset + bytes.len() as u64 >= size,
    })
}

async fn read_header(path: &Path) -> Result<Vec<u8>> {
    let file = tokio::fs::File::open(path).await?;
    let mut header = Vec::with_capacity(BINARY_SAMPLE_BYTES);
    file.take(BINARY_SAMPLE_BYTES as u64)
        .read_to_end(&mut header)
        .await?;
    Ok(header)
}

/// A size and mtime validator, used by range reads and raw downloads.
fn weak_etag(metadata: &std::fs::Metadata) -> String {
    let mut hasher = Sha256::new();
    hasher.update(metadata.len().to_le_bytes());
    if let Ok(modified) = metadata.modified() {
        if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
            hasher.update(duration.as_nanos().to_le_bytes());
        }
    }
    format!("W/\"{}\"", hex::encode(hasher.finalize()))
}

/// Return `true` when the buffer looks like binary data.
///
/// A buffer is binary when its first 8192 bytes contain a NUL byte or fail
/// UTF-8 validation. An incomplete multi-byte character at the sample
/// boundary is not treated as invalid.
pub fn is_binary(bytes: &[u8]) -> bool {
    is_binary_truncated(bytes, false)
}

/// Binary detection aware of whether the caller deliberately truncated the
/// file. A cut multi-byte character at the end of a truncated buffer is UTF-8,
/// not binary.
fn is_binary_truncated(bytes: &[u8], truncated: bool) -> bool {
    let sample_len = bytes.len().min(BINARY_SAMPLE_BYTES);
    let sample = &bytes[..sample_len];

    if sample.contains(&0) {
        return true;
    }

    match std::str::from_utf8(sample) {
        Ok(_) => false,
        Err(error) => {
            let sample_cut = bytes.len() > BINARY_SAMPLE_BYTES;
            !((truncated || sample_cut) && error.error_len().is_none())
        }
    }
}

/// Detect a display language from a path extension.
pub fn detect_language(path: &str) -> Option<&'static str> {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();

    Some(match extension.as_str() {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" => "javascript",
        "jsx" => "jsx",
        "json" => "json",
        "md" => "markdown",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "html" | "htm" => "html",
        "css" => "css",
        "sh" | "bash" => "shell",
        "sql" => "sql",
        _ => return None,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str) -> ReadFileRequest {
        ReadFileRequest {
            path: path.to_string(),
            max_bytes: None,
            etag: None,
        }
    }

    #[tokio::test]
    async fn reads_utf8_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("main.rs"), b"fn main() {}\n").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let content = read_file(&policy, &settings, &request("main.rs"))
            .await
            .expect("read")
            .expect("content");
        assert_eq!(content.content, "fn main() {}\n");
        assert_eq!(content.encoding, "utf-8");
        assert_eq!(content.language.as_deref(), Some("rust"));
        assert!(!content.is_binary);
        assert!(!content.truncated);
        assert_eq!(content.etag.len(), 64);
    }

    #[tokio::test]
    async fn etag_returns_not_modified() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"stable").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let first = read_file(&policy, &settings, &request("a.txt"))
            .await
            .expect("read")
            .expect("content");
        let mut next = request("a.txt");
        next.etag = Some(first.etag);
        let second = read_file(&policy, &settings, &next).await.expect("read");
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn truncates_large_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("big.txt"), b"0123456789").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::new(4, vec![]);

        let content = read_file(&policy, &settings, &request("big.txt"))
            .await
            .expect("read")
            .expect("content");
        assert_eq!(content.content, "0123");
        assert!(content.truncated);
        assert_eq!(content.size, 10);
    }

    #[tokio::test]
    async fn request_max_bytes_is_honored() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("big.txt"), b"0123456789").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let mut req = request("big.txt");
        req.max_bytes = Some(3);
        let content = read_file(&policy, &settings, &req)
            .await
            .expect("read")
            .expect("content");
        assert_eq!(content.content, "012");
        assert!(content.truncated);
    }

    #[tokio::test]
    async fn detects_binary_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("logo.bin"), [0u8, 159, 146, 150]).expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let content = read_file(&policy, &settings, &request("logo.bin"))
            .await
            .expect("read")
            .expect("content");
        assert!(content.is_binary);
        assert_eq!(content.encoding, "binary");
        assert!(content.content.is_empty());
        assert!(content.language.is_none());
    }

    #[tokio::test]
    async fn rejects_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let error = read_file(&policy, &settings, &request("src"))
            .await
            .expect_err("directory");
        assert!(matches!(error, WorkspaceError::InvalidRequest(_)));
    }

    #[test]
    fn binary_detection_handles_utf8_boundary() {
        // 8191 ASCII bytes then a 3-byte character cut at the sample boundary.
        let mut bytes = vec![b'a'; BINARY_SAMPLE_BYTES - 1];
        bytes.extend_from_slice(&[0xE4, 0xB8, 0xAD, b'!']);
        assert!(!is_binary(&bytes));

        let mut invalid = vec![b'a'; 10];
        invalid.push(0xFF);
        assert!(is_binary(&invalid));
        assert!(is_binary(b"abc\0def"));
        assert!(!is_binary("plain utf8 ✓".as_bytes()));
    }

    #[test]
    fn detects_languages() {
        assert_eq!(detect_language("src/lib.rs"), Some("rust"));
        assert_eq!(detect_language("app.tsx"), Some("tsx"));
        assert_eq!(detect_language("README.md"), Some("markdown"));
        assert_eq!(detect_language("data.yaml"), Some("yaml"));
        assert_eq!(detect_language("no-extension"), None);
    }

    fn range_request(path: &str, offset: u64, length: Option<u64>) -> ReadFileRangeRequest {
        ReadFileRangeRequest {
            path: path.to_string(),
            offset,
            length,
        }
    }

    fn decode(range: &FileRange) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(&range.data)
            .expect("base64")
    }

    #[tokio::test]
    async fn stat_reports_size_binary_and_a_weak_etag() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hello").expect("write");
        std::fs::write(dir.path().join("logo.bin"), [0u8, 1, 2]).expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");

        let text = stat_file(&policy, "a.txt").await.expect("stat");
        assert_eq!(text.size, 5);
        assert!(!text.is_binary);
        assert!(text.etag.starts_with("W/\""));
        assert_eq!(text.etag.len(), 3 + 64 + 1);

        let binary = stat_file(&policy, "logo.bin").await.expect("stat");
        assert!(binary.is_binary);
    }

    #[tokio::test]
    async fn range_reads_a_mid_file_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"0123456789").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let range = read_file_range(&policy, &settings, &range_request("a.txt", 3, Some(4)))
            .await
            .expect("range");
        assert_eq!(decode(&range), b"3456");
        assert_eq!(range.offset, 3);
        assert_eq!(range.total_size, 10);
        assert!(!range.eof);
        assert!(!range.is_binary);
    }

    #[tokio::test]
    async fn range_reaches_eof_and_is_capped_by_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"0123456789").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::new(4, vec![]);

        let range = read_file_range(&policy, &settings, &range_request("a.txt", 8, Some(100)))
            .await
            .expect("range");
        assert_eq!(decode(&range), b"89");
        assert!(range.eof);
    }

    #[tokio::test]
    async fn range_at_the_end_is_empty_and_eof() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"abc").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let range = read_file_range(&policy, &settings, &range_request("a.txt", 3, None))
            .await
            .expect("range");
        assert!(decode(&range).is_empty());
        assert!(range.eof);
    }

    #[tokio::test]
    async fn range_rejects_past_eof_and_zero_length() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"abc").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let past = read_file_range(&policy, &settings, &range_request("a.txt", 4, None))
            .await
            .expect_err("past eof");
        assert!(matches!(past, WorkspaceError::InvalidRequest(_)));

        let zero = read_file_range(&policy, &settings, &range_request("a.txt", 0, Some(0)))
            .await
            .expect_err("zero length");
        assert!(matches!(zero, WorkspaceError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn range_classifies_binary_files_from_the_header() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("logo.bin"), [0u8, 1, 2, 3]).expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let range = read_file_range(&policy, &settings, &range_request("logo.bin", 1, Some(2)))
            .await
            .expect("range");
        assert!(range.is_binary);
        assert_eq!(decode(&range), [1, 2]);
    }
}

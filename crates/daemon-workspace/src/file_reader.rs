//! Bounded file reading, binary detection and ETag support.

use fleet_core_protocol::workspace::{FileContent, ReadFileRequest};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

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
}

//! ACP v1 client capabilities served by Fleet.
//!
//! v1 lets the agent ask the client to read and write text files. v2 removed
//! these methods. Fleet implements the v1 filesystem requests with a strict
//! workspace boundary: absolute paths are accepted only when they resolve
//! inside the session's working directory, symlinks are resolved before the
//! check, and both reads and writes are size limited.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_client_protocol::v1::schema as v1s;
use agent_client_protocol::v1::Responder;

use super::session::AcpSessionHandle;

/// Maximum bytes Fleet returns or accepts for one file operation.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Build the v1 client capabilities Fleet advertises.
pub fn v1_client_capabilities() -> v1s::ClientCapabilities {
    v1s::ClientCapabilities::new()
        .fs(v1s::FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        .terminal(true)
        .elicitation(
            v1s::ElicitationCapabilities::new().form(v1s::ElicitationFormCapabilities::new()),
        )
}

/// Handle `fs/read_text_file`.
pub async fn handle_read_text_file(
    session: Arc<AcpSessionHandle>,
    request: v1s::ReadTextFileRequest,
    responder: Responder<v1s::ReadTextFileResponse>,
) {
    let root = session.workspace_root.lock().clone();
    match read_text_file(&root, &request.path, request.line, request.limit).await {
        Ok(content) => {
            let _ = responder.respond(v1s::ReadTextFileResponse::new(content));
        }
        Err(error) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error));
        }
    }
}

/// Handle `fs/write_text_file`.
pub async fn handle_write_text_file(
    session: Arc<AcpSessionHandle>,
    request: v1s::WriteTextFileRequest,
    responder: Responder<v1s::WriteTextFileResponse>,
) {
    let root = session.workspace_root.lock().clone();
    match write_text_file(&root, &request.path, &request.content).await {
        Ok(()) => {
            let _ = responder.respond(v1s::WriteTextFileResponse::new());
        }
        Err(error) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error));
        }
    }
}

async fn read_text_file(
    root: &str,
    requested: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> Result<String, String> {
    let path = resolve_workspace_path(Path::new(root), requested).await?;
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(format!(
            "file {} exceeds the {MAX_FILE_BYTES} byte read limit",
            path.display()
        ));
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(slice_lines(&content, line, limit))
}

async fn write_text_file(root: &str, requested: &Path, content: &str) -> Result<(), String> {
    if content.len() > MAX_FILE_BYTES {
        return Err(format!(
            "content exceeds the {MAX_FILE_BYTES} byte write limit"
        ));
    }
    let path = resolve_workspace_path(Path::new(root), requested).await?;
    tokio::fs::write(&path, content)
        .await
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Resolve `requested` and prove it stays inside `root`.
///
/// The parent directory is canonicalized before the final component is
/// checked, so a symlinked directory cannot escape the workspace. An existing
/// final component is canonicalized too, which rejects file symlinks that
/// point outside the root.
pub async fn resolve_workspace_path(root: &Path, requested: &Path) -> Result<PathBuf, String> {
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|error| format!("workspace root {} is invalid: {error}", root.display()))?;
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| format!("path {} has no parent", candidate.display()))?;
    let parent = tokio::fs::canonicalize(parent)
        .await
        .map_err(|error| format!("path {} is invalid: {error}", candidate.display()))?;
    if !parent.starts_with(&root) {
        return Err(format!(
            "path {} escapes the workspace",
            requested.display()
        ));
    }
    let name = candidate
        .file_name()
        .ok_or_else(|| format!("path {} has no file name", candidate.display()))?;
    let path = parent.join(name);
    if tokio::fs::symlink_metadata(&path).await.is_ok() {
        let canonical = tokio::fs::canonicalize(&path)
            .await
            .map_err(|error| format!("path {} is invalid: {error}", path.display()))?;
        if !canonical.starts_with(&root) {
            return Err(format!(
                "path {} escapes the workspace through a symlink",
                requested.display()
            ));
        }
        return Ok(canonical);
    }
    Ok(path)
}

/// Slice text by 1-based line number and line count, preserving line endings.
fn slice_lines(content: &str, line: Option<u32>, limit: Option<u32>) -> String {
    if line.is_none() && limit.is_none() {
        return content.to_string();
    }
    let start = line.unwrap_or(1).saturating_sub(1) as usize;
    let take = limit.unwrap_or(u32::MAX) as usize;
    content
        .split_inclusive('\n')
        .skip(start)
        .take(take)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_slicing_preserves_newlines() {
        assert_eq!(slice_lines("a\nb\nc\n", None, None), "a\nb\nc\n");
        assert_eq!(slice_lines("a\nb\nc\n", Some(2), None), "b\nc\n");
        assert_eq!(slice_lines("a\nb\nc\n", Some(2), Some(1)), "b\n");
        assert_eq!(slice_lines("a\nb", Some(2), None), "b");
        assert_eq!(slice_lines("a\nb\n", Some(9), None), "");
    }

    #[tokio::test]
    async fn paths_outside_the_workspace_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        tokio::fs::create_dir(&root).await.unwrap();
        assert!(resolve_workspace_path(&root, Path::new("/etc/passwd"))
            .await
            .is_err());
        assert!(resolve_workspace_path(&root, Path::new("../escape.txt"))
            .await
            .is_err());
        let inside = resolve_workspace_path(&root, Path::new("file.txt")).await;
        assert_eq!(
            inside.unwrap(),
            root.canonicalize().unwrap().join("file.txt")
        );
    }

    #[tokio::test]
    async fn file_symlinks_escaping_the_workspace_are_rejected() {
        #[cfg(unix)]
        {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("workspace");
            tokio::fs::create_dir(&root).await.unwrap();
            let outside = temp.path().join("outside.txt");
            tokio::fs::write(&outside, "secret").await.unwrap();
            std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
            assert!(resolve_workspace_path(&root, Path::new("link.txt"))
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn read_and_write_round_trip_inside_the_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_string_lossy().to_string();
        write_text_file(&root, Path::new("note.txt"), "hello\nworld\n")
            .await
            .unwrap();
        let content = read_text_file(&root, Path::new("note.txt"), Some(2), None)
            .await
            .unwrap();
        assert_eq!(content, "world\n");
    }

    #[tokio::test]
    async fn oversized_content_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_string_lossy().to_string();
        let content = "x".repeat(MAX_FILE_BYTES + 1);
        assert!(write_text_file(&root, Path::new("big.txt"), &content)
            .await
            .is_err());
    }
}

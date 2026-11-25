//! Line-based text search across a workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fleet_core_protocol::workspace::{SearchMatch, SearchRequest, SearchResponse};
use ignore::WalkBuilder;

use crate::file_reader::is_binary;
use crate::ignore_rules::IgnoreScope;
use crate::path_policy::PathPolicy;
use crate::settings::WorkspaceSettings;
use crate::{Result, WorkspaceError};

/// Default maximum number of matches returned.
pub(crate) const DEFAULT_MAX_RESULTS: usize = 200;

/// Hard upper bound for search results.
pub(crate) const MAX_RESULTS: usize = 1000;

/// Maximum bytes of a matched line echoed back to the client.
pub(crate) const MAX_MATCH_LINE_BYTES: usize = 1024;

/// Search text files under an optional workspace-relative subdirectory.
pub(crate) async fn search(
    policy: &PathPolicy,
    settings: &WorkspaceSettings,
    request: &SearchRequest,
) -> Result<SearchResponse> {
    if request.query.is_empty() {
        return Err(WorkspaceError::InvalidRequest(
            "search query must not be empty".to_string(),
        ));
    }

    let base = request.path.as_deref().unwrap_or("");
    let resolved = policy.resolve_existing(base).await?;
    let metadata = tokio::fs::metadata(&resolved).await?;

    let files = if metadata.is_file() {
        vec![resolved]
    } else if metadata.is_dir() {
        collect_files(policy.root().to_path_buf(), resolved, settings.clone()).await?
    } else {
        return Err(WorkspaceError::InvalidRequest(
            "search path is not a regular file or directory".to_string(),
        ));
    };

    let max_results = request
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, MAX_RESULTS);
    let case_sensitive = request.case_sensitive.unwrap_or(false);
    let needle = if case_sensitive {
        request.query.clone()
    } else {
        request.query.to_lowercase()
    };

    let mut matches = Vec::new();
    let mut truncated = false;

    'files: for file in files {
        let metadata = match tokio::fs::metadata(&file).await {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            Ok(_) => continue,
            Err(error) => {
                tracing::debug!(path = %file.display(), error = %error, "skipping unreadable file");
                continue;
            }
        };
        if metadata.len() > settings.max_file_bytes {
            continue;
        }

        let bytes = match read_capped(&file, settings.max_file_bytes).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::debug!(path = %file.display(), error = %error, "skipping unreadable file");
                continue;
            }
        };

        if is_binary(&bytes) {
            continue;
        }

        let relative = match workspace_relative(policy.root(), &file) {
            Some(relative) => relative,
            None => continue,
        };
        let text = String::from_utf8_lossy(&bytes);

        for (index, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let position = if case_sensitive {
                line.find(&needle)
            } else {
                // Fold once per line, then map the match back to the original
                // byte offset so the reported column is not shifted by
                // characters whose lowercase form has a different width.
                line.to_lowercase()
                    .find(&needle)
                    .map(|position| lowercase_offset(line, position))
            };
            if let Some(position) = position {
                if matches.len() >= max_results {
                    truncated = true;
                    break 'files;
                }
                matches.push(SearchMatch {
                    path: relative.clone(),
                    line_number: index as u64 + 1,
                    line: truncate_line(line, MAX_MATCH_LINE_BYTES),
                    column: Some(position as u64 + 1),
                });
            }
        }
    }

    Ok(SearchResponse { matches, truncated })
}

/// Read at most `limit` bytes, never loading an unbounded file into memory.
async fn read_capped(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024) as usize);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() as u64 > limit {
        bytes.truncate(limit as usize);
    }
    Ok(bytes)
}

/// Translate a byte offset in the lowercased form back to the original line.
fn lowercase_offset(line: &str, lowercase_offset: usize) -> usize {
    let mut folded = 0;
    for (index, character) in line.char_indices() {
        if folded >= lowercase_offset {
            return index;
        }
        folded += character.to_lowercase().map(char::len_utf8).sum::<usize>();
    }
    line.len()
}

/// Truncate a matched line on a UTF-8 boundary.
fn truncate_line(line: &str, max: usize) -> String {
    if line.len() <= max {
        return line.to_string();
    }
    let mut end = max;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

/// Walk `base` with the same ignore rules as a directory listing, returning
/// regular files.
///
/// The walker applies ignore files it encounters below `base`; the shared
/// [`IgnoreScope`] adds root, default and ancestor rules that a subdirectory
/// walk would otherwise miss.
async fn collect_files(
    workspace_root: PathBuf,
    base: PathBuf,
    settings: WorkspaceSettings,
) -> Result<Vec<PathBuf>> {
    tokio::task::spawn_blocking(move || {
        let scope = IgnoreScope::load(&workspace_root, &base, &settings);
        let mut builder = WalkBuilder::new(&base);
        builder
            .hidden(false)
            .parents(false)
            .git_global(false)
            .git_exclude(false)
            .git_ignore(true)
            .require_git(false)
            .ignore(true);
        // No `max_filesize`: the ignore walker checks it before the custom
        // filter, which would let oversized files bypass ignore rules. The
        // per-file size check below enforces the same limit.

        let filter_scope = Arc::clone(&scope);
        builder.filter_entry(move |entry| {
            entry.depth() == 0
                || !filter_scope.is_ignored(
                    entry.path(),
                    entry
                        .file_type()
                        .map(|file_type| file_type.is_dir())
                        .unwrap_or(false),
                )
        });

        let mut files = Vec::new();
        for result in builder.build() {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::debug!(error = %error, "skipping walk error");
                    continue;
                }
            };
            if entry.depth() == 0 {
                continue;
            }
            let is_file = entry
                .file_type()
                .map(|file_type| file_type.is_file())
                .unwrap_or(false);
            if !is_file {
                continue;
            }

            let path = entry.into_path();
            if !path.starts_with(&workspace_root) {
                continue;
            }
            files.push(path);
        }

        files.sort();
        Ok(files)
    })
    .await
    .map_err(|error| WorkspaceError::InvalidRequest(format!("search walk failed: {error}")))?
}

fn workspace_relative(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            path: None,
            max_results: None,
            case_sensitive: None,
        }
    }

    #[tokio::test]
    async fn finds_matches_case_insensitively() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(
            dir.path().join("src/main.rs"),
            b"fn main() {}\n// Needle here\nlet needle = 1;\n",
        )
        .expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let response = search(&policy, &settings, &request("NEEDLE"))
            .await
            .expect("search");
        assert_eq!(response.matches.len(), 2);
        assert!(!response.truncated);
        assert_eq!(response.matches[0].path, "src/main.rs");
        assert_eq!(response.matches[0].line_number, 2);
        assert_eq!(response.matches[1].line_number, 3);
    }

    #[tokio::test]
    async fn respects_ignore_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").expect("write");
        std::fs::write(dir.path().join("ignored.txt"), b"needle\n").expect("write");
        std::fs::write(dir.path().join("kept.txt"), b"needle\n").expect("write");
        std::fs::create_dir_all(dir.path().join("node_modules")).expect("mkdir");
        std::fs::write(dir.path().join("node_modules/pkg.txt"), b"needle\n").expect("write");

        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();
        let response = search(&policy, &settings, &request("needle"))
            .await
            .expect("search");

        assert_eq!(response.matches.len(), 1);
        assert_eq!(response.matches[0].path, "kept.txt");
    }

    #[tokio::test]
    async fn truncates_at_max_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"needle\nneedle\nneedle\n").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let mut req = request("needle");
        req.max_results = Some(2);
        let response = search(&policy, &settings, &req).await.expect("search");

        assert_eq!(response.matches.len(), 2);
        assert!(response.truncated);
    }

    #[tokio::test]
    async fn reports_columns_in_original_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "İneedle\n".as_bytes()).expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let response = search(&policy, &settings, &request("needle"))
            .await
            .expect("search");
        assert_eq!(response.matches.len(), 1);
        // `İ` is two bytes in the original line; its lowercase form is three.
        assert_eq!(response.matches[0].column, Some(3));
    }

    #[tokio::test]
    async fn skips_binary_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("bin.dat"),
            [0u8, b'n', b'e', b'e', b'd', b'l', b'e'],
        )
        .expect("write");
        std::fs::write(dir.path().join("text.txt"), b"needle").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let response = search(&policy, &settings, &request("needle"))
            .await
            .expect("search");
        assert_eq!(response.matches.len(), 1);
        assert_eq!(response.matches[0].path, "text.txt");
    }

    #[tokio::test]
    async fn applies_root_and_nested_ignore_files_in_subdirectories() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".gitignore"), "root-ignored.txt\n").expect("write");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/.gitignore"), "nested-ignored.txt\n").expect("write");
        std::fs::write(dir.path().join("src/root-ignored.txt"), b"needle").expect("write");
        std::fs::write(dir.path().join("src/nested-ignored.txt"), b"needle").expect("write");
        std::fs::write(dir.path().join("src/kept.txt"), b"needle").expect("write");

        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();
        let mut req = request("needle");
        req.path = Some("src".to_string());
        let response = search(&policy, &settings, &req).await.expect("search");

        // A subdirectory walk must honor the workspace-root ignore file as
        // well as the one inside the searched directory.
        assert_eq!(response.matches.len(), 1);
        assert_eq!(response.matches[0].path, "src/kept.txt");
    }

    #[tokio::test]
    async fn searches_a_subdirectory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::create_dir_all(dir.path().join("docs")).expect("mkdir");
        std::fs::write(dir.path().join("src/a.txt"), b"needle").expect("write");
        std::fs::write(dir.path().join("docs/b.txt"), b"needle").expect("write");
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let settings = WorkspaceSettings::default();

        let mut req = request("needle");
        req.path = Some("src".to_string());
        let response = search(&policy, &settings, &req).await.expect("search");
        assert_eq!(response.matches.len(), 1);
        assert_eq!(response.matches[0].path, "src/a.txt");
    }
}

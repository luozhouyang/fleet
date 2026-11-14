//! Line-based text search across a workspace.

use std::path::{Path, PathBuf};

use agent_fleet_protocol::workspace::{SearchMatch, SearchRequest, SearchResponse};
use ignore::WalkBuilder;

use crate::file_reader::is_binary;
use crate::ignore_rules::IgnoreRules;
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
            let haystack = if case_sensitive {
                line.to_string()
            } else {
                line.to_lowercase()
            };
            if let Some(position) = haystack.find(&needle) {
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

/// Walk `root` with gitignore and default-ignore rules, returning regular
/// files under `base`.
async fn collect_files(
    workspace_root: PathBuf,
    base: PathBuf,
    settings: WorkspaceSettings,
) -> Result<Vec<PathBuf>> {
    tokio::task::spawn_blocking(move || {
        let rules = IgnoreRules::load(&workspace_root, &settings);
        let mut builder = WalkBuilder::new(&base);
        builder
            .hidden(false)
            .parents(false)
            .git_global(false)
            .git_exclude(false)
            .git_ignore(true)
            .require_git(false)
            .ignore(true)
            .max_filesize(Some(settings.max_file_bytes));

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
            let relative = match path.strip_prefix(&workspace_root) {
                Ok(relative) => relative,
                Err(_) => continue,
            };
            if rules.has_default_component(relative) {
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

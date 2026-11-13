//! Worktree, index and commit-range diffs, including untracked files.

use std::path::{Component, Path};
use std::time::Duration;

use agent_fleet_protocol::git::{
    DiffHunk, DiffLine, DiffLineType, FileDiff, GitDiff, GitDiffOptions, GitFileStatus,
};

use crate::command::GitRunner;
use crate::diff_parser::parse_diff_files;
use crate::error::{GitError, Result};

/// Untracked files larger than this are skipped rather than synthesized.
pub(crate) const MAX_UNTRACKED_BYTES: u64 = 2 * 1024 * 1024;

/// Number of leading bytes inspected for NUL when classifying a file.
const BINARY_SNIFF_BYTES: usize = 8000;

/// Default number of context lines in a diff.
pub(crate) const DEFAULT_CONTEXT_LINES: u32 = 3;

/// Compute a unified diff for a repository.
pub(crate) async fn diff(
    runner: &GitRunner,
    root: &Path,
    options: &GitDiffOptions,
    timeout: Duration,
) -> Result<GitDiff> {
    let args = build_diff_args(options, &[])?;
    let stdout = runner.run_checked(root, &args, timeout).await?;
    let mut files = parse_diff_files(&stdout)?;

    // An explicit path is enough to request the contents of an untracked
    // file; otherwise untracked files require `include_untracked`.
    if options.include_untracked || options.path.is_some() {
        files.extend(untracked_diffs(runner, root, options.path.as_deref(), timeout).await?);
    }

    let is_binary = files.iter().any(|file| file.is_binary);
    Ok(GitDiff { files, is_binary })
}

/// Build the argument vector for a `git diff` invocation.
pub(crate) fn build_diff_args(options: &GitDiffOptions, extra: &[&str]) -> Result<Vec<String>> {
    let mut args = vec!["diff".to_string()];
    args.extend(extra.iter().map(|value| (*value).to_string()));
    args.push("--no-color".to_string());
    args.push("--no-ext-diff".to_string());

    match (&options.base, &options.head) {
        (Some(base), Some(head)) => {
            validate_revision(base)?;
            validate_revision(head)?;
            args.push(format!("{base}..{head}"));
        }
        (Some(base), None) => {
            validate_revision(base)?;
            args.push(base.clone());
        }
        (None, Some(head)) => {
            validate_revision(head)?;
            args.push(head.clone());
        }
        (None, None) => {
            if options.staged {
                args.push("--cached".to_string());
            }
        }
    }

    args.push(format!(
        "--unified={}",
        options.context_lines.unwrap_or(DEFAULT_CONTEXT_LINES)
    ));
    if options.ignore_whitespace {
        args.push("--ignore-all-space".to_string());
    }
    if let Some(path) = &options.path {
        validate_requested_path(path)?;
        args.push("--".to_string());
        args.push(path.clone());
    }

    Ok(args)
}

/// Reject revisions that could be interpreted as options or shell syntax.
pub(crate) fn validate_revision(revision: &str) -> Result<()> {
    if revision.is_empty()
        || revision.starts_with('-')
        || revision.bytes().any(|byte| byte == 0)
        || revision
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(GitError::InvalidPath(format!(
            "invalid revision `{revision}`"
        )));
    }
    Ok(())
}

/// Reject absolute paths, parent traversal and option-like paths.
pub(crate) fn validate_requested_path(path: &str) -> Result<()> {
    if path.is_empty() || path.starts_with('-') || path.bytes().any(|byte| byte == 0) {
        return Err(GitError::InvalidPath(format!("invalid path `{path}`")));
    }
    Path::new(path)
        .components()
        .try_for_each(|component| match component {
            Component::Normal(_) | Component::CurDir => Ok(()),
            _ => Err(GitError::InvalidPath(format!(
                "path escapes the repository: `{path}`"
            ))),
        })
}

/// Read untracked files and synthesize added-file diffs.
pub(crate) async fn untracked_diffs(
    runner: &GitRunner,
    root: &Path,
    path_filter: Option<&str>,
    timeout: Duration,
) -> Result<Vec<FileDiff>> {
    let mut args = vec![
        "ls-files".to_string(),
        "--others".to_string(),
        "--exclude-standard".to_string(),
        "-z".to_string(),
    ];
    if let Some(path) = path_filter {
        validate_requested_path(path)?;
        args.push("--".to_string());
        args.push(path.to_string());
    }

    let stdout = runner.run_checked(root, &args, timeout).await?;
    let canonical_root = std::fs::canonicalize(root)?;

    let mut files = Vec::new();
    for record in stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(record);
        if let Some(file) = read_untracked_file(&canonical_root, &path) {
            files.push(file);
        }
    }
    Ok(files)
}

/// Read one untracked file, applying root, size and binary checks.
fn read_untracked_file(canonical_root: &Path, path: &str) -> Option<FileDiff> {
    let relative = Path::new(path);
    if relative.is_absolute() {
        return None;
    }
    for component in relative.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => return None,
        }
    }

    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let parent = std::fs::canonicalize(canonical_root.join(parent_relative)).ok()?;
    if !parent.starts_with(canonical_root) {
        return None;
    }

    let file_name = relative.file_name()?;
    let candidate = parent.join(file_name);
    let metadata = std::fs::symlink_metadata(&candidate).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_UNTRACKED_BYTES {
        return None;
    }

    // Resolve the file itself as well so symlinked directories cannot escape.
    let canonical_file = std::fs::canonicalize(&candidate).ok()?;
    if !canonical_file.starts_with(canonical_root) {
        return None;
    }

    let bytes = std::fs::read(&canonical_file).ok()?;
    let display_path = path.to_string();

    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0)
        || std::str::from_utf8(&bytes).is_err()
    {
        return Some(FileDiff {
            path: display_path,
            old_path: None,
            status: GitFileStatus::Added,
            additions: 0,
            deletions: 0,
            is_binary: true,
            hunks: Vec::new(),
        });
    }

    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.split_terminator('\n').collect();
    let additions = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let hunk = DiffHunk {
        old_start: 0,
        old_lines: 0,
        new_start: if lines.is_empty() { 0 } else { 1 },
        new_lines: lines.len() as u64,
        header: None,
        lines: lines
            .iter()
            .enumerate()
            .map(|(index, line)| DiffLine {
                line_type: DiffLineType::Added,
                content: (*line).to_string(),
                old_line_no: None,
                new_line_no: Some(index as u64 + 1),
            })
            .collect(),
    };

    Some(FileDiff {
        path: display_path,
        old_path: None,
        status: GitFileStatus::Added,
        additions,
        deletions: 0,
        is_binary: false,
        hunks: vec![hunk],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        assert!(validate_requested_path("../etc/passwd").is_err());
        assert!(validate_requested_path("/etc/passwd").is_err());
        assert!(validate_requested_path("-rf").is_err());
        assert!(validate_requested_path("src/lib.rs").is_ok());
        assert!(validate_requested_path("./src/lib.rs").is_ok());
        assert!(validate_requested_path("").is_err());
    }

    #[test]
    fn rejects_option_like_revisions() {
        assert!(validate_revision("--output=/tmp/x").is_err());
        assert!(validate_revision("main").is_ok());
        assert!(validate_revision("main..feature").is_ok());
    }

    #[test]
    fn builds_worktree_diff_args() {
        let options = GitDiffOptions::default();
        let args = build_diff_args(&options, &[]).expect("args");
        assert_eq!(
            args,
            vec!["diff", "--no-color", "--no-ext-diff", "--unified=3",]
        );
    }

    #[test]
    fn builds_staged_range_args() {
        let options = GitDiffOptions {
            staged: true,
            base: Some("main".into()),
            head: Some("HEAD".into()),
            context_lines: Some(1),
            ignore_whitespace: true,
            path: Some("src/lib.rs".into()),
            ..GitDiffOptions::default()
        };
        let args = build_diff_args(&options, &["--numstat", "-z"]).expect("args");
        assert_eq!(args[1], "--numstat");
        assert!(args.contains(&"main..HEAD".to_string()));
        assert!(!args.contains(&"--cached".to_string()));
        assert!(args.contains(&"--unified=1".to_string()));
        assert!(args.contains(&"--ignore-all-space".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("src/lib.rs"));
        assert_eq!(args[args.len() - 2], "--");
    }
}

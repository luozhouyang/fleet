//! Worktree, index and commit-range diffs, including untracked files.

use std::io::Read;
use std::path::{Component, Path};
use std::time::Duration;

use fleet_core_common::paths::{normalize_relative, PathRules};
use fleet_core_protocol::git::{
    DiffHunk, DiffLine, DiffLineType, FileDiff, GitDiff, GitDiffOptions, GitFileStatus, GitRawDiff,
};

use crate::command::{command_failure, GitRunner};
use crate::diff_parser::{parse_diff_files, MAX_FILE_DIFF_CHARS, MAX_FILE_DIFF_LINES};
use crate::error::{GitError, Result};

/// Untracked files larger than this are skipped rather than synthesized.
pub(crate) const MAX_UNTRACKED_BYTES: u64 = 2 * 1024 * 1024;

/// Aggregate bytes read from untracked files in one request.
pub(crate) const MAX_UNTRACKED_TOTAL_BYTES: usize = 8 * 1024 * 1024;

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
    let options = resolve_options(runner, root, options, timeout).await?;
    let args = build_diff_args(&options, &[])?;
    let stdout = runner.run_checked(root, &args, timeout).await?;
    let mut files = parse_diff_files(&stdout)?;

    // Untracked files only belong to a worktree-vs-index diff, never to the
    // index or a commit range.
    let mut untracked_truncated = false;
    if options.include_untracked
        && !options.staged
        && options.base.is_none()
        && options.head.is_none()
    {
        let (untracked, truncated) =
            untracked_diffs(runner, root, options.path.as_deref(), timeout, true).await?;
        files.extend(untracked);
        untracked_truncated = truncated;
    }

    let truncated = truncate_large_files(&mut files) || untracked_truncated;
    let is_binary = files.iter().any(|file| file.is_binary);
    Ok(GitDiff {
        files,
        is_binary,
        truncated,
    })
}

/// Raw `git diff` output, used as the complete fallback for a truncated
/// structured diff.
pub(crate) async fn raw_diff(
    runner: &GitRunner,
    root: &Path,
    options: &GitDiffOptions,
    timeout: Duration,
) -> Result<GitRawDiff> {
    let options = resolve_options(runner, root, options, timeout).await?;
    let args = build_diff_args(&options, &[])?;
    // Raw diffs are explicitly the escape hatch for oversized output, so the
    // capture cap marks the result truncated instead of failing the request.
    let output = runner.run_allow_truncated(root, &args, timeout).await?;
    if !output.success {
        return Err(command_failure(root, &args, &output));
    }
    Ok(GitRawDiff {
        patch: String::from_utf8_lossy(&output.stdout).into_owned(),
        truncated: output.truncated,
    })
}

/// Cap each file's hunks for the structured response.
///
/// A file past either limit keeps the leading hunks and is marked
/// `truncated`; the caller can fetch `git.diff.raw` for the complete patch.
fn truncate_large_files(files: &mut [FileDiff]) -> bool {
    let mut any = false;
    for file in files.iter_mut() {
        if file.is_binary {
            continue;
        }
        let mut lines = 0usize;
        let mut chars = 0usize;
        let mut kept = Vec::new();
        let mut truncated = false;
        for mut hunk in std::mem::take(&mut file.hunks) {
            let mut kept_lines = Vec::new();
            for line in std::mem::take(&mut hunk.lines) {
                if lines >= MAX_FILE_DIFF_LINES || chars + line.content.len() > MAX_FILE_DIFF_CHARS
                {
                    truncated = true;
                    break;
                }
                lines += 1;
                chars += line.content.len();
                kept_lines.push(line);
            }
            hunk.lines = kept_lines;
            if !hunk.lines.is_empty() {
                kept.push(hunk);
            }
            if truncated {
                break;
            }
        }
        file.hunks = kept;
        // The parser already enforces the same caps; never clear its flag.
        file.truncated |= truncated;
        any |= file.truncated;
    }
    any
}

/// Resolve base/head revisions to commit hashes and validate the path.
pub(crate) async fn resolve_options(
    runner: &GitRunner,
    root: &Path,
    options: &GitDiffOptions,
    timeout: Duration,
) -> Result<GitDiffOptions> {
    let mut resolved = options.clone();
    if let Some(base) = &options.base {
        resolved.base = Some(resolve_commit(runner, root, base, timeout).await?);
    }
    if let Some(head) = &options.head {
        resolved.head = Some(resolve_commit(runner, root, head, timeout).await?);
    }
    if let Some(path) = &options.path {
        validate_requested_path(path)?;
    }
    Ok(resolved)
}

/// Resolve a user-supplied revision to a full commit hash.
pub(crate) async fn resolve_commit(
    runner: &GitRunner,
    root: &Path,
    revision: &str,
    timeout: Duration,
) -> Result<String> {
    validate_revision(revision)?;
    let args = vec![
        "rev-parse".to_string(),
        "--verify".to_string(),
        "--end-of-options".to_string(),
        format!("{revision}^{{commit}}"),
    ];
    let stdout = runner
        .run_checked(root, &args, timeout)
        .await
        .map_err(|error| {
            GitError::InvalidPath(format!("unknown revision `{revision}`: {error}"))
        })?;
    let hash = String::from_utf8_lossy(&stdout).trim().to_string();
    if !(40..=64).contains(&hash.len()) || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(GitError::InvalidPath(format!(
            "revision `{revision}` did not resolve to a commit"
        )));
    }
    Ok(hash)
}

/// Build the argument vector for a `git diff` invocation.
pub(crate) fn build_diff_args(options: &GitDiffOptions, extra: &[&str]) -> Result<Vec<String>> {
    let mut args = vec!["diff".to_string()];
    args.extend(extra.iter().map(|value| (*value).to_string()));
    args.push("--no-color".to_string());
    args.push("--no-ext-diff".to_string());
    args.push("--relative".to_string());

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
    // Always constrain the diff to the workspace subtree. Git reports paths
    // relative to the current prefix because of `--relative`.
    args.push("--".to_string());
    args.push(options.path.clone().unwrap_or_else(|| ".".to_string()));

    Ok(args)
}

/// Reject revisions that could be interpreted as options or shell syntax.
pub(crate) fn validate_revision(revision: &str) -> Result<()> {
    if revision.is_empty()
        || revision.starts_with('-')
        || revision.starts_with('.')
        || revision.contains("..")
        || revision.contains(':')
        || revision.contains('\\')
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
    normalize_relative(path, PathRules::GIT_PATHSPEC)
        .map(|_| ())
        .map_err(|rejection| GitError::InvalidPath(format!("invalid path `{path}`: {rejection}")))
}

/// Read untracked files and synthesize added-file diffs.
///
/// `materialize` controls whether line content is copied into [`DiffLine`]
/// values. Summaries need only the counts, so they skip that allocation. The
/// returned flag reports whether the aggregate budget cut the file list.
pub(crate) async fn untracked_diffs(
    runner: &GitRunner,
    root: &Path,
    path_filter: Option<&str>,
    timeout: Duration,
    materialize: bool,
) -> Result<(Vec<FileDiff>, bool)> {
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
    let mut remaining = MAX_UNTRACKED_TOTAL_BYTES;
    let mut truncated = false;
    for record in stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        if remaining == 0 {
            truncated = true;
            break;
        }
        let path = String::from_utf8_lossy(record);
        if let Some((file, charged)) =
            read_untracked_file(&canonical_root, &path, remaining, materialize)
        {
            remaining = remaining.saturating_sub(charged);
            truncated |= file.truncated;
            files.push(file);
        }
    }
    Ok((files, truncated))
}

/// Read one untracked file, applying root, size and binary checks.
///
/// At most `budget` bytes are read (and reported back) so one large file or
/// many small files cannot exhaust memory before the response cap applies.
fn read_untracked_file(
    canonical_root: &Path,
    path: &str,
    budget: usize,
    materialize: bool,
) -> Option<(FileDiff, usize)> {
    if budget == 0 {
        return None;
    }
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

    // The metadata check can race a growing file; cap the read itself too.
    let limit = MAX_UNTRACKED_BYTES.min(budget as u64);
    let mut file = std::fs::File::open(&canonical_file).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    let oversized = bytes.len() as u64 > limit;
    if oversized {
        bytes.truncate(limit as usize);
    }
    let charged = bytes.len();
    let display_path = path.to_string();

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => std::borrow::Cow::Borrowed(text),
        // The read cap can split a multi-byte character; a trailing
        // incomplete sequence still means text, not binary.
        Err(error) if oversized && error.error_len().is_none() => {
            std::borrow::Cow::Owned(String::from_utf8_lossy(&bytes).into_owned())
        }
        Err(_) => {
            return Some((
                FileDiff {
                    path: display_path,
                    old_path: None,
                    status: GitFileStatus::Added,
                    additions: 0,
                    deletions: 0,
                    is_binary: true,
                    truncated: oversized,
                    hunks: Vec::new(),
                },
                charged,
            ))
        }
    };
    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
        return Some((
            FileDiff {
                path: display_path,
                old_path: None,
                status: GitFileStatus::Added,
                additions: 0,
                deletions: 0,
                is_binary: true,
                truncated: oversized,
                hunks: Vec::new(),
            },
            charged,
        ));
    }

    let total_lines = text.split_terminator('\n').count();
    let additions = u32::try_from(total_lines).unwrap_or(u32::MAX);
    let mut kept_lines = Vec::new();
    let mut kept_bytes = 0usize;
    let mut truncated = oversized;
    if materialize {
        for line in text.split_terminator('\n') {
            if kept_lines.len() >= MAX_FILE_DIFF_LINES
                || kept_bytes.saturating_add(line.len()) > MAX_FILE_DIFF_CHARS
            {
                truncated = true;
                break;
            }
            kept_bytes += line.len();
            kept_lines.push(DiffLine {
                line_type: DiffLineType::Added,
                content: line.to_string(),
                old_line_no: None,
                new_line_no: Some(kept_lines.len() as u64 + 1),
            });
        }
    }

    let hunks = if materialize {
        vec![DiffHunk {
            old_start: 0,
            old_lines: 0,
            new_start: if total_lines == 0 { 0 } else { 1 },
            new_lines: total_lines as u64,
            header: None,
            lines: kept_lines,
        }]
    } else {
        Vec::new()
    };

    Some((
        FileDiff {
            path: display_path,
            old_path: None,
            status: GitFileStatus::Added,
            additions,
            deletions: 0,
            is_binary: false,
            truncated,
            hunks,
        },
        charged,
    ))
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
        assert!(validate_revision("main..feature").is_err());
    }

    #[test]
    fn builds_worktree_diff_args() {
        let options = GitDiffOptions::default();
        let args = build_diff_args(&options, &[]).expect("args");
        assert_eq!(
            args,
            vec![
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--relative",
                "--unified=3",
                "--",
                ".",
            ]
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

    fn file_with_lines(count: usize) -> FileDiff {
        FileDiff {
            path: "a.txt".into(),
            old_path: None,
            status: GitFileStatus::Modified,
            additions: count as u32,
            deletions: 0,
            is_binary: false,
            truncated: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 0,
                new_start: 1,
                new_lines: count as u64,
                header: None,
                lines: (0..count)
                    .map(|index| DiffLine {
                        line_type: DiffLineType::Added,
                        content: format!("line {index}"),
                        old_line_no: None,
                        new_line_no: Some(index as u64 + 1),
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn truncates_files_past_the_line_limit() {
        let mut files = vec![file_with_lines(MAX_FILE_DIFF_LINES + 1)];
        assert!(truncate_large_files(&mut files));
        assert!(files[0].truncated);
        assert_eq!(
            files[0]
                .hunks
                .iter()
                .map(|hunk| hunk.lines.len())
                .sum::<usize>(),
            MAX_FILE_DIFF_LINES
        );
    }

    #[test]
    fn leaves_small_files_untouched() {
        let mut files = vec![file_with_lines(3)];
        assert!(!truncate_large_files(&mut files));
        assert!(!files[0].truncated);
        assert_eq!(files[0].hunks[0].lines.len(), 3);
    }

    #[test]
    fn untracked_reader_caps_materialized_lines() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let mut content = String::new();
        for index in 0..MAX_FILE_DIFF_LINES + 10 {
            content.push_str(&format!("line {index}\n"));
        }
        std::fs::write(root.join("big.txt"), content).unwrap();

        let (file, _) =
            read_untracked_file(&root, "big.txt", MAX_UNTRACKED_TOTAL_BYTES, true).unwrap();
        assert!(file.truncated);
        assert_eq!(file.additions as usize, MAX_FILE_DIFF_LINES + 10);
        assert_eq!(file.hunks[0].lines.len(), MAX_FILE_DIFF_LINES);
    }

    #[test]
    fn untracked_reader_respects_the_byte_budget() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        std::fs::write(root.join("a.txt"), "abcdefghij").unwrap();

        let (file, charged) = read_untracked_file(&root, "a.txt", 4, true).unwrap();
        assert_eq!(charged, 4);
        assert!(file.truncated);
        assert_eq!(file.additions, 1);
        assert_eq!(file.hunks[0].lines[0].content, "abcd");
    }

    #[test]
    fn untracked_summary_skips_line_materialization() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let (file, _) =
            read_untracked_file(&root, "a.txt", MAX_UNTRACKED_TOTAL_BYTES, false).unwrap();
        assert_eq!(file.additions, 3);
        assert!(file.hunks.is_empty());
        assert!(!file.truncated);
    }
}

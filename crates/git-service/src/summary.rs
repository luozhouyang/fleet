//! Per-file diff summaries.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use agent_fleet_protocol::git::{
    DiffSummary, GitDiffOptions, GitDiffSummary, GitFileStatus, GitStatus,
};

use crate::command::GitRunner;
use crate::diff::{build_diff_args, resolve_options, untracked_diffs};
use crate::error::{GitError, Result};
use crate::status::read_status;

/// One parsed `--numstat -z` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NumStat {
    additions: u32,
    deletions: u32,
    is_binary: bool,
    path: String,
    old_path: Option<String>,
}

/// Produce a summary of the changed files in a repository.
pub(crate) async fn diff_summary(
    runner: &GitRunner,
    root: &Path,
    options: &GitDiffOptions,
    status_timeout: Duration,
    diff_timeout: Duration,
) -> Result<GitDiffSummary> {
    let status = read_status(runner, root, status_timeout).await?;
    let options = resolve_options(runner, root, options, diff_timeout).await?;
    let is_range = options.base.is_some() || options.head.is_some();

    let numstat_args = summary_args(&options, &["--numstat", "-z"])?;
    let stdout = runner
        .run_checked(root, &numstat_args, diff_timeout)
        .await?;
    let stats = parse_numstat(&stdout)?;

    let lookup = if is_range {
        let args = summary_args(&options, &["--name-status", "-z"])?;
        let stdout = runner.run_checked(root, &args, diff_timeout).await?;
        parse_name_status(&stdout)?
    } else {
        status_lookup(&status, options.staged)
    };

    let mut files = Vec::with_capacity(stats.len());
    for stat in stats {
        let (file_status, mapped_old) = lookup
            .get(&stat.path)
            .cloned()
            .unwrap_or((GitFileStatus::Modified, None));
        files.push(DiffSummary {
            path: stat.path,
            old_path: stat.old_path.or(mapped_old),
            status: file_status,
            additions: stat.additions,
            deletions: stat.deletions,
            is_binary: stat.is_binary,
            staged: options.staged,
        });
    }

    if options.include_untracked && !options.staged && !is_range {
        for file in untracked_diffs(runner, root, options.path.as_deref(), diff_timeout).await? {
            files.push(DiffSummary {
                path: file.path,
                old_path: None,
                status: GitFileStatus::Untracked,
                additions: file.additions,
                deletions: file.deletions,
                is_binary: file.is_binary,
                staged: false,
            });
        }
    }

    Ok(GitDiffSummary {
        files,
        branch: status.branch,
        ahead: status.ahead,
        behind: status.behind,
    })
}

/// Build diff arguments for a summary command.
///
/// `--unified=N` is removed because supplying it alongside `--numstat` or
/// `--name-status` makes Git emit a patch in addition to the summary.
fn summary_args(options: &GitDiffOptions, extra: &[&str]) -> Result<Vec<String>> {
    let mut args = build_diff_args(options, extra)?;
    args.retain(|argument| !argument.starts_with("--unified="));
    Ok(args)
}

/// Build a status lookup for the requested side (index or worktree).
fn status_lookup(
    status: &GitStatus,
    staged: bool,
) -> HashMap<String, (GitFileStatus, Option<String>)> {
    status
        .files
        .iter()
        .filter(|file| {
            file.staged == staged
                && file.status != GitFileStatus::Untracked
                && file.status != GitFileStatus::Ignored
        })
        .map(|file| (file.path.clone(), (file.status, file.old_path.clone())))
        .collect()
}

/// Parse `git diff --numstat -z` output.
fn parse_numstat(bytes: &[u8]) -> Result<Vec<NumStat>> {
    let records: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();

    let mut stats = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        let fields: Vec<&[u8]> = record.splitn(3, |byte| *byte == b'\t').collect();
        if fields.len() != 3 {
            return Err(GitError::InvalidOutput(format!(
                "malformed numstat record: {}",
                String::from_utf8_lossy(record)
            )));
        }

        let (additions, added_binary) = parse_count(fields[0], record)?;
        let (deletions, deleted_binary) = parse_count(fields[1], record)?;

        let (path, old_path, consumed) = if fields[2].is_empty() {
            let old = records.get(index + 1).ok_or_else(|| {
                GitError::InvalidOutput("numstat rename record is missing its old path".to_string())
            })?;
            let new = records.get(index + 2).ok_or_else(|| {
                GitError::InvalidOutput("numstat rename record is missing its new path".to_string())
            })?;
            (decode(new), Some(decode(old)), 3)
        } else {
            (decode(fields[2]), None, 1)
        };

        stats.push(NumStat {
            additions,
            deletions,
            is_binary: added_binary || deleted_binary,
            path,
            old_path,
        });
        index += consumed;
    }
    Ok(stats)
}

/// Parse an additions/deletions field. `-` marks a binary file.
fn parse_count(field: &[u8], record: &[u8]) -> Result<(u32, bool)> {
    if field == b"-" {
        return Ok((0, true));
    }
    let text = std::str::from_utf8(field).map_err(|_| {
        GitError::InvalidOutput(format!(
            "malformed numstat count in record: {}",
            String::from_utf8_lossy(record)
        ))
    })?;
    let value = text.parse::<u32>().map_err(|_| {
        GitError::InvalidOutput(format!(
            "malformed numstat count in record: {}",
            String::from_utf8_lossy(record)
        ))
    })?;
    Ok((value, false))
}

/// Parse `git diff --name-status -z` output into a path lookup.
fn parse_name_status(bytes: &[u8]) -> Result<HashMap<String, (GitFileStatus, Option<String>)>> {
    let records: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();

    let mut lookup = HashMap::new();
    let mut index = 0;
    while index < records.len() {
        let token = records[index];
        let letter = *token
            .first()
            .ok_or_else(|| GitError::InvalidOutput("empty name-status record".to_string()))?;
        let (path, old_path, consumed) = match letter {
            b'R' | b'C' => {
                let old = records.get(index + 1).ok_or_else(|| {
                    GitError::InvalidOutput(
                        "name-status rename is missing its old path".to_string(),
                    )
                })?;
                let new = records.get(index + 2).ok_or_else(|| {
                    GitError::InvalidOutput(
                        "name-status rename is missing its new path".to_string(),
                    )
                })?;
                (decode(new), Some(decode(old)), 3)
            }
            _ => {
                let path = records.get(index + 1).ok_or_else(|| {
                    GitError::InvalidOutput("name-status record is missing its path".to_string())
                })?;
                (decode(path), None, 2)
            }
        };
        lookup.insert(path, (map_name_status(letter), old_path));
        index += consumed;
    }
    Ok(lookup)
}

/// Map a `--name-status` letter to the protocol enum.
fn map_name_status(letter: u8) -> GitFileStatus {
    match letter {
        b'A' => GitFileStatus::Added,
        b'D' => GitFileStatus::Deleted,
        b'R' => GitFileStatus::Renamed,
        b'C' => GitFileStatus::Copied,
        b'U' => GitFileStatus::Conflicted,
        _ => GitFileStatus::Modified,
    }
}

/// Decode a path lossily.
fn decode(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat_records() {
        let bytes = b"1\t2\tsrc/a.rs\0-\t-\timage.png\0";
        let stats = parse_numstat(bytes).expect("parse");
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].path, "src/a.rs");
        assert_eq!(stats[0].additions, 1);
        assert_eq!(stats[0].deletions, 2);
        assert!(!stats[0].is_binary);
        assert!(stats[1].is_binary);
    }

    #[test]
    fn parses_numstat_rename() {
        let bytes = b"0\t0\t\0old name.txt\0new name.txt\0";
        let stats = parse_numstat(bytes).expect("parse");
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].path, "new name.txt");
        assert_eq!(stats[0].old_path.as_deref(), Some("old name.txt"));
    }

    #[test]
    fn parses_name_status_rename_and_modify() {
        let bytes = b"R100\0old.txt\0new.txt\0M\0src/lib.rs\0";
        let lookup = parse_name_status(bytes).expect("parse");
        assert_eq!(
            lookup.get("new.txt"),
            Some(&(GitFileStatus::Renamed, Some("old.txt".to_string())))
        );
        assert_eq!(
            lookup.get("src/lib.rs"),
            Some(&(GitFileStatus::Modified, None))
        );
    }

    #[test]
    fn rejects_malformed_numstat() {
        let error = parse_numstat(b"oops\0").expect_err("must fail");
        assert!(matches!(error, GitError::InvalidOutput(_)));
    }
}

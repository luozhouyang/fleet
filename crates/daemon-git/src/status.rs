//! Parsing of `git status --porcelain=v2 -z --branch` output.

use std::path::Path;
use std::time::Duration;

use fleet_core_protocol::git::{GitFileChange, GitFileStatus, GitStatus};

use crate::command::GitRunner;
use crate::error::{GitError, Result};

/// Record prefixes understood by the porcelain v2 parser.
const RECORD_PREFIXES: [&[u8]; 6] = [b"1 ", b"2 ", b"u ", b"? ", b"! ", b"# "];

/// Read the status of the repository containing `root`.
pub(crate) async fn read_status(
    runner: &GitRunner,
    root: &Path,
    timeout: Duration,
) -> Result<GitStatus> {
    // Restrict output to the workspace subtree and rewrite repo-relative
    // paths to workspace-relative paths. Without the pathspec, a workspace
    // rooted at a repository subdirectory sees (and leaks) siblings.
    let prefix_args = vec!["rev-parse".to_string(), "--show-prefix".to_string()];
    let prefix = String::from_utf8_lossy(&runner.run_checked(root, &prefix_args, timeout).await?)
        .trim()
        .trim_end_matches('/')
        .to_string();
    let args = vec![
        "status".to_string(),
        "--porcelain=v2".to_string(),
        "-z".to_string(),
        "--branch".to_string(),
        "--".to_string(),
        ".".to_string(),
    ];
    let stdout = runner.run_checked(root, &args, timeout).await?;
    let mut status = parse_porcelain_v2(&stdout)?;
    if !prefix.is_empty() {
        let strip = format!("{prefix}/");
        for file in &mut status.files {
            file.path = file
                .path
                .strip_prefix(&strip)
                .unwrap_or(&file.path)
                .to_string();
            if let Some(old) = &file.old_path {
                file.old_path = Some(old.strip_prefix(&strip).unwrap_or(old).to_string());
            }
        }
    }
    Ok(status)
}

/// Parse porcelain v2 output.
///
/// Records are NUL-separated. Paths are decoded lossily when they are not
/// valid UTF-8. Both the canonical NUL-separated rename form and the
/// tab-separated rename form are accepted.
pub fn parse_porcelain_v2(bytes: &[u8]) -> Result<GitStatus> {
    let mut status = GitStatus::clean();
    let mut saw_branch_head = false;

    let mut records: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    // A trailing NUL produces a final empty record.
    while records.last().is_some_and(|record| record.is_empty()) {
        records.pop();
    }

    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.is_empty() {
            index += 1;
            continue;
        }

        if let Some(header) = record.strip_prefix(b"# ") {
            parse_header(header, &mut status, &mut saw_branch_head)?;
            index += 1;
            continue;
        }

        if record.starts_with(b"1 ") {
            let fields = split_fields(record, 9)?;
            let (index_status, worktree_status) = split_xy(fields[1])?;
            let path = decode_path(fields[8]);
            push_sides(&mut status.files, path, None, index_status, worktree_status);
            index += 1;
            continue;
        }

        if record.starts_with(b"2 ") {
            let fields = split_fields(record, 10)?;
            let (index_status, worktree_status) = split_xy(fields[1])?;
            let (path, old_path, consumed) = parse_rename_path(&records, index, fields[9])?;
            push_sides(
                &mut status.files,
                path,
                old_path,
                index_status,
                worktree_status,
            );
            index += 1 + consumed;
            continue;
        }

        if record.starts_with(b"u ") {
            let fields = split_fields(record, 11)?;
            let (index_status, worktree_status) = split_xy(fields[1])?;
            status.files.push(GitFileChange {
                path: decode_path(fields[10]),
                old_path: None,
                status: GitFileStatus::Conflicted,
                staged: false,
                index_status: Some(char_text(index_status)),
                worktree_status: Some(char_text(worktree_status)),
            });
            index += 1;
            continue;
        }

        if let Some(path) = record.strip_prefix(b"? ") {
            status.files.push(GitFileChange {
                path: decode_path(path),
                old_path: None,
                status: GitFileStatus::Untracked,
                staged: false,
                index_status: None,
                worktree_status: Some("?".to_string()),
            });
            index += 1;
            continue;
        }

        if let Some(path) = record.strip_prefix(b"! ") {
            status.files.push(GitFileChange {
                path: decode_path(path),
                old_path: None,
                status: GitFileStatus::Ignored,
                staged: false,
                index_status: None,
                worktree_status: Some("!".to_string()),
            });
            index += 1;
            continue;
        }

        return Err(GitError::InvalidOutput(format!(
            "unknown porcelain v2 record: {}",
            String::from_utf8_lossy(record)
        )));
    }

    if !saw_branch_head {
        // A clean detached checkout may omit no header, but the field is only
        // meaningful when the header was present.
        status.detached = None;
    }
    status.is_clean = status.files.is_empty();
    Ok(status)
}

/// Parse one `# ` header record (without its prefix).
fn parse_header(header: &[u8], status: &mut GitStatus, saw_branch_head: &mut bool) -> Result<()> {
    let header = String::from_utf8_lossy(header);
    if let Some(oid) = header.strip_prefix("branch.oid ") {
        if oid != "(initial)" && !oid.is_empty() {
            status.head_oid = Some(oid.to_string());
        }
        return Ok(());
    }
    if let Some(head) = header.strip_prefix("branch.head ") {
        *saw_branch_head = true;
        if head == "(detached)" {
            status.branch = None;
            status.detached = Some(true);
        } else {
            status.branch = Some(head.to_string());
            status.detached = Some(false);
        }
        return Ok(());
    }
    if let Some(ab) = header.strip_prefix("branch.ab ") {
        let mut parts = ab.split_whitespace();
        let ahead = parts
            .next()
            .and_then(|value| value.strip_prefix('+'))
            .and_then(|value| value.parse::<u32>().ok());
        let behind = parts
            .next()
            .and_then(|value| value.strip_prefix('-'))
            .and_then(|value| value.parse::<u32>().ok());
        match (ahead, behind) {
            (Some(ahead), Some(behind)) => {
                status.ahead = ahead;
                status.behind = behind;
            }
            _ => {
                return Err(GitError::InvalidOutput(format!(
                    "malformed branch.ab header: {ab}"
                )));
            }
        }
        return Ok(());
    }
    // `branch.upstream` and any future headers carry no field in `GitStatus`.
    Ok(())
}

/// Split a rename record into its new path, original path and number of
/// additional records consumed.
fn parse_rename_path<'a>(
    records: &[&'a [u8]],
    index: usize,
    path_field: &'a [u8],
) -> Result<(String, Option<String>, usize)> {
    let tab_split = path_field
        .iter()
        .position(|byte| *byte == b'\t')
        .map(|position| (&path_field[..position], &path_field[position + 1..]));

    if let Some((path, inline_original)) = tab_split {
        // In the canonical `-z` form the path may itself contain a tab and the
        // original path is a separate record. Prefer that interpretation when
        // the following record does not look like a new status record.
        let next_is_original = records
            .get(index + 1)
            .is_some_and(|next| !next.is_empty() && !looks_like_record(next));
        if next_is_original {
            let original = decode_path(records[index + 1]);
            return Ok((decode_path(path_field), Some(original), 1));
        }
        return Ok((decode_path(path), Some(decode_path(inline_original)), 0));
    }

    let original = records.get(index + 1).ok_or_else(|| {
        GitError::InvalidOutput("rename record is missing its original path".to_string())
    })?;
    Ok((decode_path(path_field), Some(decode_path(original)), 1))
}

/// Whether a record begins with a known porcelain v2 record prefix.
fn looks_like_record(record: &[u8]) -> bool {
    RECORD_PREFIXES
        .iter()
        .any(|prefix| record.starts_with(prefix))
}

/// Split a record into at most `count` space-separated fields.
fn split_fields(record: &[u8], count: usize) -> Result<Vec<&[u8]>> {
    let fields: Vec<&[u8]> = record.splitn(count, |byte| *byte == b' ').collect();
    if fields.len() != count {
        return Err(GitError::InvalidOutput(format!(
            "expected {count} fields in porcelain v2 record: {}",
            String::from_utf8_lossy(record)
        )));
    }
    Ok(fields)
}

/// Split the two-character XY status field.
fn split_xy(field: &[u8]) -> Result<(u8, u8)> {
    if field.len() != 2 {
        return Err(GitError::InvalidOutput(format!(
            "malformed XY status field: {}",
            String::from_utf8_lossy(field)
        )));
    }
    Ok((field[0], field[1]))
}

/// Emit one change per meaningful side of an XY status pair.
fn push_sides(
    files: &mut Vec<GitFileChange>,
    path: String,
    old_path: Option<String>,
    index_status: u8,
    worktree_status: u8,
) {
    let index_raw = char_text(index_status);
    let worktree_raw = char_text(worktree_status);

    if index_status != b'.' && index_status != b' ' {
        files.push(GitFileChange {
            path: path.clone(),
            old_path: old_path.clone(),
            status: map_status(index_status),
            staged: true,
            index_status: Some(index_raw.clone()),
            worktree_status: Some(worktree_raw.clone()),
        });
    }
    if worktree_status != b'.' && worktree_status != b' ' {
        files.push(GitFileChange {
            path,
            old_path,
            status: map_status(worktree_status),
            staged: false,
            index_status: Some(index_raw),
            worktree_status: Some(worktree_raw),
        });
    }
}

/// Map a porcelain status character to the protocol enum.
fn map_status(byte: u8) -> GitFileStatus {
    match byte {
        b'A' => GitFileStatus::Added,
        b'D' => GitFileStatus::Deleted,
        b'R' => GitFileStatus::Renamed,
        b'C' => GitFileStatus::Copied,
        b'U' => GitFileStatus::Conflicted,
        b'?' => GitFileStatus::Untracked,
        b'!' => GitFileStatus::Ignored,
        // `M`, type changes (`T`) and anything unexpected are modifications.
        _ => GitFileStatus::Modified,
    }
}

/// Decode a path lossily.
fn decode_path(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Render a status byte as a one-character string.
fn char_text(byte: u8) -> String {
    (byte as char).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nul(records: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend_from_slice(record.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn parses_branch_and_ahead_behind() {
        let bytes = nul(&[
            "# branch.oid abc123",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
        ]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head_oid.as_deref(), Some("abc123"));
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
        assert_eq!(status.detached, Some(false));
        assert!(status.is_clean);
        assert!(status.files.is_empty());
    }

    #[test]
    fn parses_detached_head() {
        let bytes = nul(&["# branch.oid deadbeef", "# branch.head (detached)"]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert!(status.branch.is_none());
        assert_eq!(status.detached, Some(true));
    }

    #[test]
    fn parses_both_sides_of_a_modified_file() {
        let bytes = nul(&[
            "# branch.oid abc",
            "# branch.head main",
            "1 MM N... 100644 100644 100644 abc def file with space.txt",
        ]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert_eq!(status.files.len(), 2);
        assert!(status.files[0].staged);
        assert!(!status.files[1].staged);
        assert_eq!(status.files[0].path, "file with space.txt");
        assert_eq!(status.files[0].status, GitFileStatus::Modified);
        assert_eq!(status.files[0].index_status.as_deref(), Some("M"));
    }

    #[test]
    fn parses_tab_separated_rename() {
        let bytes = nul(&[
            "# branch.head main",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new name.txt\told name.txt",
        ]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert_eq!(status.files.len(), 1);
        assert!(status.files[0].staged);
        assert_eq!(status.files[0].status, GitFileStatus::Renamed);
        assert_eq!(status.files[0].path, "new name.txt");
        assert_eq!(status.files[0].old_path.as_deref(), Some("old name.txt"));
    }

    #[test]
    fn parses_nul_separated_rename() {
        let bytes = nul(&[
            "# branch.head main",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new name.txt",
            "old name.txt",
            "? untracked.txt",
        ]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert_eq!(status.files.len(), 2);
        assert_eq!(status.files[0].status, GitFileStatus::Renamed);
        assert_eq!(status.files[0].old_path.as_deref(), Some("old name.txt"));
        assert_eq!(status.files[1].status, GitFileStatus::Untracked);
    }

    #[test]
    fn parses_conflicted_and_untracked_records() {
        let bytes = nul(&[
            "# branch.head main",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.txt",
            "? new.txt",
            "! ignored.txt",
        ]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert_eq!(status.files.len(), 3);
        assert_eq!(status.files[0].status, GitFileStatus::Conflicted);
        assert_eq!(status.files[1].status, GitFileStatus::Untracked);
        assert_eq!(status.files[2].status, GitFileStatus::Ignored);
        assert!(!status.is_clean);
    }

    #[test]
    fn parses_typechange_as_modified() {
        let bytes = nul(&[
            "# branch.head main",
            "1 .T N... 100644 100644 100644 aaa bbb link",
        ]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].status, GitFileStatus::Modified);
        assert!(!status.files[0].staged);
    }

    #[test]
    fn rejects_malformed_records() {
        let bytes = nul(&["1 M"]);
        let error = parse_porcelain_v2(&bytes).expect_err("must fail");
        assert!(matches!(error, GitError::InvalidOutput(_)));
    }

    #[test]
    fn initial_repository_has_no_head_oid() {
        let bytes = nul(&["# branch.oid (initial)", "# branch.head main"]);
        let status = parse_porcelain_v2(&bytes).expect("parse");
        assert!(status.head_oid.is_none());
        assert_eq!(status.branch.as_deref(), Some("main"));
    }
}

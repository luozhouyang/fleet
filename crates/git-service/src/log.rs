//! Commit history.

use std::path::Path;
use std::time::Duration;

use agent_fleet_protocol::git::{GitLogEntry, GitLogRequest};

use crate::command::GitRunner;
use crate::diff::{validate_requested_path, validate_revision};
use crate::error::{GitError, Result};

/// Default number of commits returned by [`log`].
const DEFAULT_LOG_LIMIT: usize = 50;

/// Maximum number of commits that can be requested at once.
const MAX_LOG_LIMIT: usize = 500;

/// Unit separator used to delimit `git log` fields.
const FIELD_SEPARATOR: char = '\u{1f}';

/// Format string used for `git log`.
const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%ct%x1f%s%x1f%P%x1f%D";

/// Return recent commits, optionally limited to a reference or path.
pub(crate) async fn log(
    runner: &GitRunner,
    root: &Path,
    request: &GitLogRequest,
    timeout: Duration,
) -> Result<Vec<GitLogEntry>> {
    let limit = request
        .limit
        .unwrap_or(DEFAULT_LOG_LIMIT)
        .clamp(1, MAX_LOG_LIMIT);

    let mut args = vec![
        "log".to_string(),
        "-n".to_string(),
        limit.to_string(),
        "--no-color".to_string(),
        "--no-ext-diff".to_string(),
        LOG_FORMAT.to_string(),
    ];

    if let Some(reference) = &request.reference {
        validate_revision(reference)?;
        args.push(reference.clone());
    }
    if let Some(path) = &request.path {
        validate_requested_path(path)?;
        args.push("--".to_string());
        args.push(path.clone());
    }

    let stdout = runner.run_checked(root, &args, timeout).await?;
    parse_log(&stdout)
}

/// Parse unit-separated `git log` output.
fn parse_log(bytes: &[u8]) -> Result<Vec<GitLogEntry>> {
    let text = String::from_utf8_lossy(bytes);
    let mut entries = Vec::new();

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.splitn(8, FIELD_SEPARATOR);
        let id = next_field(&mut fields, line)?;
        let short_id = next_field(&mut fields, line)?;
        let author = next_field(&mut fields, line)?;
        let email = next_field(&mut fields, line)?;
        let timestamp = next_field(&mut fields, line)?;
        let message = next_field(&mut fields, line)?;
        let parents = next_field(&mut fields, line)?;
        let refs = next_field(&mut fields, line)?;

        let seconds = timestamp.parse::<i64>().map_err(|_| {
            GitError::InvalidOutput(format!("invalid commit timestamp `{timestamp}`"))
        })?;
        let timestamp = chrono::DateTime::from_timestamp(seconds, 0).ok_or_else(|| {
            GitError::InvalidOutput(format!("commit timestamp out of range: {seconds}"))
        })?;

        entries.push(GitLogEntry {
            id: id.to_string(),
            short_id: short_id.to_string(),
            message: message.to_string(),
            author: author.to_string(),
            email: email.to_string(),
            timestamp,
            parents: parents.split_whitespace().map(str::to_string).collect(),
            refs: refs
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect(),
        });
    }
    Ok(entries)
}

/// Take the next field or report a malformed line.
fn next_field<'a>(fields: &mut impl Iterator<Item = &'a str>, line: &str) -> Result<&'a str> {
    fields
        .next()
        .ok_or_else(|| GitError::InvalidOutput(format!("malformed git log record: {line}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_entry() {
        let bytes = b"abc123\x1fabc\x1fAda\x1fada@example.com\x1f1700000000\x1fInitial commit\x1fdef456 ghi789\x1fHEAD -> main, origin/main\n";
        let entries = parse_log(bytes).expect("parse");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.id, "abc123");
        assert_eq!(entry.short_id, "abc");
        assert_eq!(entry.author, "Ada");
        assert_eq!(entry.email, "ada@example.com");
        assert_eq!(entry.message, "Initial commit");
        assert_eq!(entry.timestamp.timestamp(), 1_700_000_000);
        assert_eq!(entry.parents, vec!["def456", "ghi789"]);
        assert_eq!(entry.refs, vec!["HEAD -> main", "origin/main"]);
    }

    #[test]
    fn parses_root_commit_without_parents_or_refs() {
        let bytes = b"abc\x1fabc\x1fA\x1fa@b.c\x1f0\x1froot\x1f\x1f\n";
        let entries = parse_log(bytes).expect("parse");
        assert!(entries[0].parents.is_empty());
        assert!(entries[0].refs.is_empty());
    }

    #[test]
    fn malformed_line_is_an_error() {
        let error = parse_log(b"only-one-field\n").expect_err("must fail");
        assert!(matches!(error, GitError::InvalidOutput(_)));
    }
}

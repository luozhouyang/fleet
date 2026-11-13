//! Branch listing.

use std::path::Path;
use std::time::Duration;

use agent_fleet_protocol::git::GitBranch;

use crate::command::GitRunner;
use crate::error::{GitError, Result};

/// Format string used for `git for-each-ref`.
const BRANCH_FORMAT: &str = "--format=%(refname)%09%(refname:short)%09%(HEAD)%09%(upstream:short)%09%(objectname)%09%(upstream:track)";

/// List local and remote branches.
pub(crate) async fn branches(
    runner: &GitRunner,
    root: &Path,
    timeout: Duration,
) -> Result<Vec<GitBranch>> {
    let args = vec![
        "for-each-ref".to_string(),
        BRANCH_FORMAT.to_string(),
        "refs/heads".to_string(),
        "refs/remotes".to_string(),
    ];
    let stdout = runner.run_checked(root, &args, timeout).await?;
    let text = String::from_utf8_lossy(&stdout);

    let mut branches = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 5 {
            return Err(GitError::InvalidOutput(format!(
                "malformed for-each-ref record: {line}"
            )));
        }

        let refname = fields[0];
        let name = fields[1];
        let is_remote = refname.starts_with("refs/remotes/");
        if name.is_empty() || (is_remote && name.ends_with("/HEAD")) {
            continue;
        }

        let (ahead, behind) = fields
            .get(5)
            .map(|track| parse_ahead_behind(track))
            .unwrap_or((0, 0));
        branches.push(GitBranch {
            name: name.to_string(),
            is_current: fields[2].trim() == "*",
            is_remote,
            upstream: non_empty(fields[3]),
            ahead,
            behind,
            oid: non_empty(fields[4]),
        });
    }
    Ok(branches)
}

/// Convert an empty field into `None`.
fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Parse `[ahead 1, behind 2]` style upstream tracking information.
fn parse_ahead_behind(track: &str) -> (u32, u32) {
    let mut ahead = 0;
    let mut behind = 0;
    let track = track.trim_matches(|character| character == '[' || character == ']');
    for part in track.split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("ahead ") {
            ahead = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = part.strip_prefix("behind ") {
            behind = value.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracking_information() {
        assert_eq!(parse_ahead_behind("[ahead 1, behind 2]"), (1, 2));
        assert_eq!(parse_ahead_behind("[behind 3]"), (0, 3));
        assert_eq!(parse_ahead_behind("[ahead 4]"), (4, 0));
        assert_eq!(parse_ahead_behind("[gone]"), (0, 0));
        assert_eq!(parse_ahead_behind(""), (0, 0));
    }

    #[test]
    fn empty_fields_are_none() {
        assert_eq!(non_empty(""), None);
        assert_eq!(non_empty("x"), Some("x".to_string()));
    }
}

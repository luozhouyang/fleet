//! Write operations: stage, unstage, commit and checkout.
//!
//! Every user-provided value is validated before Git sees it, and commands are
//! still executed as argument vectors with `--` separators so pathspecs can
//! never be interpreted as options.

use std::path::{Component, Path};
use std::time::Duration;

use agent_fleet_protocol::git::{
    GitCheckoutOptions, GitCheckoutResult, GitCommitOptions, GitCommitResult,
};

use crate::command::{command_failure, GitRunner};
use crate::error::{GitError, Result};

/// Maximum number of paths accepted in one operation.
const MAX_PATHS: usize = 1_000;

/// Maximum commit message size in bytes.
const MAX_COMMIT_MESSAGE_BYTES: usize = 64 * 1024;

/// Maximum branch name size in bytes.
const MAX_BRANCH_BYTES: usize = 255;

/// Stage the given workspace-relative paths.
pub(crate) async fn stage(
    runner: &GitRunner,
    root: &Path,
    paths: &[String],
    timeout: Duration,
) -> Result<()> {
    validate_paths(paths)?;
    let mut args = vec!["add".to_string(), "--".to_string()];
    args.extend(paths.iter().cloned());
    runner.run_checked(root, &args, timeout).await?;
    Ok(())
}

/// Unstage the given workspace-relative paths.
///
/// Requires Git 2.23+ (`git restore --staged`).
pub(crate) async fn unstage(
    runner: &GitRunner,
    root: &Path,
    paths: &[String],
    timeout: Duration,
) -> Result<()> {
    validate_paths(paths)?;
    let mut args = vec![
        "restore".to_string(),
        "--staged".to_string(),
        "--".to_string(),
    ];
    args.extend(paths.iter().cloned());
    runner.run_checked(root, &args, timeout).await?;
    Ok(())
}

/// Create (or amend) a commit.
pub(crate) async fn commit(
    runner: &GitRunner,
    root: &Path,
    options: &GitCommitOptions,
    timeout: Duration,
) -> Result<GitCommitResult> {
    let message = options.message.trim();
    if message.is_empty() {
        return Err(GitError::InvalidPath(
            "commit message must not be empty".to_string(),
        ));
    }
    if message.len() > MAX_COMMIT_MESSAGE_BYTES {
        return Err(GitError::InvalidPath(format!(
            "commit message exceeds {MAX_COMMIT_MESSAGE_BYTES} bytes"
        )));
    }
    if let Some(paths) = options.paths.as_deref() {
        validate_paths(paths)?;
    }

    let mut args = vec![
        "commit".to_string(),
        "--no-gpg-sign".to_string(),
        "-m".to_string(),
        message.to_string(),
    ];
    if options.amend {
        args.push("--amend".to_string());
    }
    if let Some(paths) = options.paths.as_deref() {
        args.push("--".to_string());
        args.extend(paths.iter().cloned());
    }

    let output = runner.run(root, &args, timeout).await?;
    if !output.success {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .to_ascii_lowercase();
        if combined.contains("nothing to commit")
            || combined.contains("no changes added to commit")
            || combined.contains("nothing added to commit")
        {
            return Err(GitError::Conflict("nothing to commit".to_string()));
        }
        return Err(command_failure(root, &args, &output));
    }

    let commit = head_revision(runner, root, timeout).await?;
    let branch = current_branch(runner, root, timeout).await?;
    let summary = message.lines().next().unwrap_or_default().to_string();
    Ok(GitCommitResult {
        commit,
        branch,
        summary,
    })
}

/// Switch to an existing branch or create a new one.
pub(crate) async fn checkout(
    runner: &GitRunner,
    root: &Path,
    options: &GitCheckoutOptions,
    timeout: Duration,
) -> Result<GitCheckoutResult> {
    validate_branch(&options.branch)?;
    let args = if options.create {
        vec![
            "switch".to_string(),
            "-c".to_string(),
            options.branch.clone(),
        ]
    } else {
        vec!["switch".to_string(), options.branch.clone()]
    };

    let output = runner.run(root, &args, timeout).await?;
    if !output.success {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .to_ascii_lowercase();
        if combined.contains("would be overwritten")
            || combined.contains("local changes")
            || combined.contains("not possible to checkout")
        {
            return Err(GitError::Conflict(
                "working tree changes conflict with the target branch".to_string(),
            ));
        }
        return Err(command_failure(root, &args, &output));
    }

    let branch = current_branch(runner, root, timeout).await?;
    Ok(GitCheckoutResult { branch })
}

async fn head_revision(runner: &GitRunner, root: &Path, timeout: Duration) -> Result<String> {
    let args = vec!["rev-parse".to_string(), "HEAD".to_string()];
    let stdout = runner.run_checked(root, &args, timeout).await?;
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

async fn current_branch(runner: &GitRunner, root: &Path, timeout: Duration) -> Result<String> {
    let args = vec![
        "rev-parse".to_string(),
        "--abbrev-ref".to_string(),
        "HEAD".to_string(),
    ];
    let stdout = runner.run_checked(root, &args, timeout).await?;
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

/// Reject paths that could escape the repository, target Git internals or be
/// parsed as options/pathspec magic.
pub(crate) fn validate_paths(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err(GitError::InvalidPath(
            "at least one path is required".to_string(),
        ));
    }
    if paths.len() > MAX_PATHS {
        return Err(GitError::InvalidPath(format!(
            "at most {MAX_PATHS} paths are accepted"
        )));
    }
    for path in paths {
        validate_one_path(path)?;
    }
    Ok(())
}

fn validate_one_path(path: &str) -> Result<()> {
    let invalid = |reason: &str| {
        Err(GitError::InvalidPath(format!(
            "invalid path `{path}`: {reason}"
        )))
    };
    if path.trim().is_empty() {
        return invalid("empty path");
    }
    if path.contains('\0') {
        return invalid("NUL byte");
    }
    if path.starts_with('-') {
        return invalid("must not start with `-`");
    }
    if path.starts_with(':') {
        return invalid("pathspec magic is not allowed");
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return invalid("absolute paths are not allowed");
    }
    for component in candidate.components() {
        match component {
            Component::ParentDir => return invalid("`..` is not allowed"),
            Component::Prefix(_) | Component::RootDir => {
                return invalid("absolute paths are not allowed")
            }
            Component::Normal(part) if part == ".git" => {
                return invalid("the `.git` directory is not accessible")
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate a branch name before passing it to Git.
pub(crate) fn validate_branch(branch: &str) -> Result<()> {
    let invalid = |reason: &str| {
        Err(GitError::InvalidPath(format!(
            "invalid branch `{branch}`: {reason}"
        )))
    };
    if branch.trim().is_empty() {
        return invalid("empty name");
    }
    if branch.len() > MAX_BRANCH_BYTES {
        return invalid("name is too long");
    }
    if branch.starts_with('-') {
        return invalid("must not start with `-`");
    }
    if branch.starts_with('/') || branch.ends_with('/') || branch.ends_with('.') {
        return invalid("invalid leading/trailing character");
    }
    if branch.ends_with(".lock") {
        return invalid("must not end with `.lock`");
    }
    if branch.contains("..") || branch.contains("@{") || branch.contains("//") {
        return invalid("contains a forbidden sequence");
    }
    if branch.chars().any(|character| {
        character.is_control()
            || character.is_whitespace()
            || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return invalid("contains a forbidden character");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn path_validation_accepts_normal_relative_paths() {
        validate_paths(&paths(&["a.txt", "src/main.rs", ".", "dir/with space.txt"]))
            .expect("valid paths");
    }

    #[test]
    fn path_validation_rejects_escapes_and_options() {
        for candidate in [
            "/etc/passwd",
            "../outside",
            "src/../../outside",
            "-c",
            ":(glob)**/x",
            "",
            ".git/config",
            "sub/.git",
        ] {
            assert!(
                validate_paths(&[candidate.to_string()]).is_err(),
                "`{candidate}` should be rejected"
            );
        }
    }

    #[test]
    fn path_validation_rejects_empty_and_oversized_lists() {
        assert!(validate_paths(&[]).is_err());
        let too_many: Vec<String> = (0..=MAX_PATHS).map(|index| format!("f{index}")).collect();
        assert!(validate_paths(&too_many).is_err());
    }

    #[test]
    fn branch_validation_accepts_common_names() {
        for name in ["main", "feature/git-writes", "fix-123", "a.b.c"] {
            validate_branch(name).expect("valid branch");
        }
    }

    #[test]
    fn branch_validation_rejects_dangerous_names() {
        for name in [
            "", "-x", "a..b", "a b", "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b", "a@{b",
            "a//b", "/a", "a/", "a.", "a.lock", "a\nb",
        ] {
            assert!(
                validate_branch(name).is_err(),
                "`{name}` should be rejected"
            );
        }
    }
}

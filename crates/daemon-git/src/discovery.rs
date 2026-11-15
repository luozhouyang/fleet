//! Repository discovery.
//!
//! Discovery relies on `git rev-parse`, which understands ordinary
//! repositories as well as linked worktrees where `.git` is a file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fleet_core_protocol::git::GitRepository;

use crate::command::{command_failure, GitRunner};
use crate::error::{GitError, Result};

/// Discover the repository containing `root`.
///
/// Returns `Ok(None)` when the directory is not inside a Git work tree. The
/// directory itself must exist and be a directory.
pub(crate) async fn discover(
    runner: &GitRunner,
    root: &Path,
    timeout: Duration,
) -> Result<Option<GitRepository>> {
    let metadata = std::fs::metadata(root)?;
    if !metadata.is_dir() {
        return Err(GitError::InvalidPath(format!(
            "not a directory: {}",
            root.display()
        )));
    }

    let args = vec![
        "rev-parse".to_string(),
        "--show-toplevel".to_string(),
        "--absolute-git-dir".to_string(),
        "--is-inside-work-tree".to_string(),
    ];
    let output = runner.run(root, &args, timeout).await?;
    if !output.success {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("not a git repository")
            || stderr.contains("not a git repo")
            || stderr.contains("must be run in a work tree")
            || stderr.contains("this operation must be run in a work tree")
        {
            return Ok(None);
        }
        return Err(command_failure(root, &args, &output));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.is_empty());
    let toplevel = lines.next().ok_or_else(|| {
        GitError::InvalidOutput("rev-parse did not report a work tree root".to_string())
    })?;
    let git_dir = lines.next().ok_or_else(|| {
        GitError::InvalidOutput("rev-parse did not report an absolute git directory".to_string())
    })?;
    let inside = lines.next().ok_or_else(|| {
        GitError::InvalidOutput("rev-parse did not report work tree membership".to_string())
    })?;

    if inside != "true" {
        return Ok(None);
    }

    let toplevel = PathBuf::from(toplevel);
    let root_field = relative_repository_root(root, &toplevel);
    Ok(Some(GitRepository {
        root: root_field,
        git_dir: git_dir.to_string(),
        worktree: Some(toplevel.to_string_lossy().replace('\\', "/")),
    }))
}

/// Express `toplevel` relative to the workspace `root`, using `/` separators.
fn relative_repository_root(root: &Path, toplevel: &Path) -> String {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let toplevel = std::fs::canonicalize(toplevel).unwrap_or_else(|_| toplevel.to_path_buf());

    if toplevel == root {
        return ".".to_string();
    }
    if let Ok(relative) = toplevel.strip_prefix(&root) {
        let relative = relative.to_string_lossy().replace('\\', "/");
        return if relative.is_empty() {
            ".".to_string()
        } else {
            relative
        };
    }
    if root.starts_with(&toplevel) {
        let depth = root
            .components()
            .count()
            .saturating_sub(toplevel.components().count());
        return std::iter::repeat_n("..", depth)
            .collect::<Vec<_>>()
            .join("/");
    }
    toplevel.to_string_lossy().replace('\\', "/")
}

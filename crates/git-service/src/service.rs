//! The public Git service facade.

use std::path::Path;
use std::time::Duration;

use agent_fleet_protocol::git::{
    GitBranch, GitCheckoutOptions, GitCheckoutResult, GitCommitOptions, GitCommitResult, GitDiff,
    GitDiffOptions, GitDiffSummary, GitLogEntry, GitLogRequest, GitRepository, GitStatus,
};

use crate::branches;
use crate::command::GitRunner;
use crate::diff;
use crate::discovery;
use crate::error::Result;
use crate::log;
use crate::status;
use crate::summary;
use crate::write;

/// Default cap on captured Git output (16 MiB per stream).
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Default timeout for status, discovery and branch commands.
pub const DEFAULT_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for diff, summary and log commands.
pub const DEFAULT_DIFF_TIMEOUT: Duration = Duration::from_secs(30);

/// Resource limits applied to every Git invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitLimits {
    /// Maximum bytes captured from each output stream.
    pub max_output_bytes: usize,
    /// Timeout for status-like commands.
    pub status_timeout: Duration,
    /// Timeout for diff-like commands.
    pub diff_timeout: Duration,
}

impl Default for GitLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            status_timeout: DEFAULT_STATUS_TIMEOUT,
            diff_timeout: DEFAULT_DIFF_TIMEOUT,
        }
    }
}

/// Read-only access to Git repositories through the `git` CLI.
///
/// Every command runs with an explicit working directory, an argument vector
/// (never a shell), a timeout and a bounded output buffer. User-provided paths
/// and revisions are validated before they are passed to Git.
#[derive(Debug, Clone, Default)]
pub struct GitService {
    limits: GitLimits,
}

impl GitService {
    /// Create a service with the default limits.
    pub fn new() -> Self {
        Self {
            limits: GitLimits::default(),
        }
    }

    /// Create a service with a custom output cap and timeout.
    ///
    /// The timeout is used for every command, including diffs and logs. Use
    /// [`GitService::new`] to keep the default 10 second status timeout and
    /// 30 second diff timeout.
    pub fn with_limits(max_output_bytes: usize, timeout: Duration) -> Self {
        Self {
            limits: GitLimits {
                max_output_bytes,
                status_timeout: timeout,
                diff_timeout: timeout,
            },
        }
    }

    /// The configured limits.
    pub fn limits(&self) -> GitLimits {
        self.limits
    }

    fn runner(&self) -> GitRunner {
        GitRunner::new(self.limits.max_output_bytes)
    }

    /// Discover the repository containing `root`.
    ///
    /// Returns `None` when `root` is not inside a Git work tree.
    ///
    /// # Errors
    ///
    /// Returns an error when `root` does not exist or is not a directory, or
    /// when Git itself fails unexpectedly.
    pub async fn discover(&self, root: &Path) -> Result<Option<GitRepository>> {
        discovery::discover(&self.runner(), root, self.limits.status_timeout).await
    }

    /// Read the status of the repository containing `root`.
    ///
    /// # Errors
    ///
    /// Returns [`GitError::NotARepository`](crate::GitError::NotARepository)
    /// when `root` is not inside a Git work tree.
    pub async fn status(&self, root: &Path) -> Result<GitStatus> {
        status::read_status(&self.runner(), root, self.limits.status_timeout).await
    }

    /// Compute a unified diff.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid revisions or paths, non-repository roots,
    /// command failures, timeouts and unparseable output.
    pub async fn diff(&self, root: &Path, options: &GitDiffOptions) -> Result<GitDiff> {
        diff::diff(&self.runner(), root, options, self.limits.diff_timeout).await
    }

    /// Compute a per-file diff summary.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid revisions or paths, non-repository roots,
    /// command failures, timeouts and unparseable output.
    pub async fn diff_summary(
        &self,
        root: &Path,
        options: &GitDiffOptions,
    ) -> Result<GitDiffSummary> {
        summary::diff_summary(
            &self.runner(),
            root,
            options,
            self.limits.status_timeout,
            self.limits.diff_timeout,
        )
        .await
    }

    /// List local and remote branches.
    ///
    /// # Errors
    ///
    /// Returns an error for non-repository roots, command failures, timeouts
    /// and unparseable output.
    pub async fn branches(&self, root: &Path) -> Result<Vec<GitBranch>> {
        branches::branches(&self.runner(), root, self.limits.status_timeout).await
    }

    /// List recent commits.
    ///
    /// The limit defaults to 50 and is clamped to the range 1 through 500.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid references or paths, non-repository roots,
    /// command failures, timeouts and unparseable output.
    pub async fn log(&self, root: &Path, request: &GitLogRequest) -> Result<Vec<GitLogEntry>> {
        log::log(&self.runner(), root, request, self.limits.diff_timeout).await
    }

    /// Stage workspace-relative paths.
    ///
    /// # Errors
    ///
    /// Rejects empty lists, absolute paths, `..`, `.git` and pathspec magic.
    pub async fn stage(&self, root: &Path, paths: &[String]) -> Result<()> {
        write::stage(&self.runner(), root, paths, self.limits.diff_timeout).await
    }

    /// Unstage workspace-relative paths (`git restore --staged`).
    ///
    /// # Errors
    ///
    /// Rejects the same path shapes as [`GitService::stage`].
    pub async fn unstage(&self, root: &Path, paths: &[String]) -> Result<()> {
        write::unstage(&self.runner(), root, paths, self.limits.diff_timeout).await
    }

    /// Create or amend a commit.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized messages and invalid paths, and reports a
    /// [`GitError::Conflict`](crate::GitError::Conflict) when there is nothing
    /// to commit.
    pub async fn commit(&self, root: &Path, options: &GitCommitOptions) -> Result<GitCommitResult> {
        write::commit(&self.runner(), root, options, self.limits.diff_timeout).await
    }

    /// Switch to (or create) a branch.
    ///
    /// # Errors
    ///
    /// Rejects invalid branch names and reports a
    /// [`GitError::Conflict`](crate::GitError::Conflict) when local changes
    /// would be overwritten.
    pub async fn checkout(
        &self,
        root: &Path,
        options: &GitCheckoutOptions,
    ) -> Result<GitCheckoutResult> {
        write::checkout(&self.runner(), root, options, self.limits.diff_timeout).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits() {
        let service = GitService::new();
        let limits = service.limits();
        assert_eq!(limits.max_output_bytes, 16 * 1024 * 1024);
        assert_eq!(limits.status_timeout, Duration::from_secs(10));
        assert_eq!(limits.diff_timeout, Duration::from_secs(30));
    }

    #[test]
    fn custom_limits() {
        let service = GitService::with_limits(1024, Duration::from_secs(2));
        let limits = service.limits();
        assert_eq!(limits.max_output_bytes, 1024);
        assert_eq!(limits.status_timeout, Duration::from_secs(2));
        assert_eq!(limits.diff_timeout, Duration::from_secs(2));
    }
}

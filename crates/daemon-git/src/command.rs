//! Git command execution.
//!
//! Every invocation uses an argument vector with an explicit working
//! directory; no shell is involved and no user-provided value is interpolated
//! into a command string.

use std::path::Path;
use std::time::Duration;

use fleet_daemon_process::{run_capture, CapturedOutput, CommandSpec};

use crate::error::{GitError, Result};

/// Maximum number of stderr bytes echoed in a command failure message.
const MAX_ERROR_MESSAGE_BYTES: usize = 4096;

/// Runs `git` with common safety options.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GitRunner {
    /// Maximum bytes captured from each output stream.
    pub(crate) max_output_bytes: usize,
}

impl GitRunner {
    /// Create a runner with the given output cap.
    pub(crate) fn new(max_output_bytes: usize) -> Self {
        Self { max_output_bytes }
    }

    /// Run Git and return its captured output regardless of exit status.
    pub(crate) async fn run(
        &self,
        root: &Path,
        args: &[String],
        timeout: Duration,
    ) -> Result<CapturedOutput> {
        let spec = CommandSpec::new("git")
            // `--no-color` and `--no-ext-diff` are not accepted by every
            // subcommand (notably `status`), so color is disabled globally and
            // the diff-specific options are added by the diff command builders.
            .args(["-c", "color.ui=false", "--no-pager", "--no-optional-locks"])
            .args(args.iter().cloned())
            .cwd(root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            // Never let the daemon's own environment redirect Git to a
            // different repository, index or object store than `root`.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .env_remove("GIT_COMMON_DIR");

        let output = run_capture(spec, timeout, self.max_output_bytes).await?;
        if output.truncated {
            return Err(GitError::InvalidOutput(format!(
                "git output exceeded {} bytes",
                self.max_output_bytes
            )));
        }
        Ok(output)
    }

    /// Run Git and return stdout, turning a non-zero exit into an error.
    pub(crate) async fn run_checked(
        &self,
        root: &Path,
        args: &[String],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let output = self.run(root, args, timeout).await?;
        if !output.success {
            return Err(command_failure(root, args, &output));
        }
        Ok(output.stdout)
    }
}

/// Convert a failed Git invocation into a descriptive error.
pub(crate) fn command_failure(root: &Path, args: &[String], output: &CapturedOutput) -> GitError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if is_not_a_repository(stderr) {
        return GitError::NotARepository(root.to_path_buf());
    }

    let mut message = if stderr.is_empty() {
        format!(
            "git {} exited with status {:?}",
            args.join(" "),
            output.status
        )
    } else {
        let stderr = if stderr.len() > MAX_ERROR_MESSAGE_BYTES {
            &stderr[..floor_char_boundary(stderr, MAX_ERROR_MESSAGE_BYTES)]
        } else {
            stderr
        };
        format!("git {} failed: {stderr}", args.join(" "))
    };
    if output.stdout.len() < MAX_ERROR_MESSAGE_BYTES {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stdout = stdout.trim();
        if !stdout.is_empty() {
            message.push_str(" (stdout: ");
            message.push_str(stdout);
            message.push(')');
        }
    }
    GitError::CommandFailed(message)
}

/// Whether Git reported that the working directory is not a repository.
fn is_not_a_repository(stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    stderr.contains("not a git repository") || stderr.contains("not a git repo")
}

/// Return the largest byte index `<= max` that is a character boundary.
fn floor_char_boundary(text: &str, max: usize) -> usize {
    let mut index = max.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

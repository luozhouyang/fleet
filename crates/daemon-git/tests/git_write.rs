//! Write-operation integration tests against a real `git` binary.

use std::path::Path;
use std::process::{Command, Output};

use fleet_core_protocol::git::{GitCheckoutOptions, GitCommitOptions};
use fleet_daemon_git::{GitError, GitService};

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

macro_rules! require_git {
    () => {
        if !git_available() {
            eprintln!("skipping test: `git` is not available");
            return;
        }
    };
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn init() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().expect("create temp dir"),
        };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Write Test"]);
        repo.git(&["config", "user.email", "write@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn output(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(self.path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("run git")
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, content).expect("write file");
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn status_porcelain(&self) -> String {
        self.git(&["status", "--porcelain"])
    }

    fn current_branch(&self) -> String {
        self.git(&["rev-parse", "--abbrev-ref", "HEAD"])
    }
}

fn paths(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

#[tokio::test]
async fn stage_and_unstage_paths() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit_all("init");
    repo.write("a.txt", "two\n");
    repo.write("new.txt", "new\n");

    let git = GitService::new();
    git.stage(repo.path(), &paths(&["a.txt", "new.txt"]))
        .await
        .expect("stage");
    let staged = repo.git(&["diff", "--cached", "--name-only"]);
    assert!(staged.contains("a.txt"));
    assert!(staged.contains("new.txt"));

    git.unstage(repo.path(), &paths(&["a.txt", "new.txt"]))
        .await
        .expect("unstage");
    assert!(repo.git(&["diff", "--cached", "--name-only"]).is_empty());
    assert!(repo.status_porcelain().contains("a.txt"));
}

#[tokio::test]
async fn stage_dot_stages_the_whole_worktree() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit_all("init");
    repo.write("b.txt", "two\n");

    let git = GitService::new();
    git.stage(repo.path(), &paths(&["."]))
        .await
        .expect("stage .");
    let staged = repo.git(&["diff", "--cached", "--name-only"]);
    assert!(staged.contains("b.txt"));
}

#[tokio::test]
async fn commit_records_message_and_returns_revision() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit_all("init");
    repo.write("a.txt", "two\n");
    repo.git(&["add", "a.txt"]);

    let git = GitService::new();
    let result = git
        .commit(
            repo.path(),
            &GitCommitOptions {
                message: "update a\n\nbody".to_string(),
                paths: None,
                amend: false,
            },
        )
        .await
        .expect("commit");
    assert_eq!(result.commit, repo.git(&["rev-parse", "HEAD"]));
    assert_eq!(result.branch, repo.current_branch());
    assert_eq!(result.summary, "update a");
    assert_eq!(repo.git(&["log", "-1", "--pretty=%B"]), "update a\n\nbody");
}

#[tokio::test]
async fn single_path_commit_leaves_other_staged_files() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.write("b.txt", "one\n");
    repo.commit_all("init");
    repo.write("a.txt", "two\n");
    repo.write("b.txt", "two\n");
    repo.git(&["add", "-A"]);

    let git = GitService::new();
    git.commit(
        repo.path(),
        &GitCommitOptions {
            message: "only a".to_string(),
            paths: Some(paths(&["a.txt"])),
            amend: false,
        },
    )
    .await
    .expect("commit a");

    let staged = repo.git(&["diff", "--cached", "--name-only"]);
    assert!(!staged.contains("a.txt"));
    assert!(staged.contains("b.txt"), "b.txt should still be staged");
}

#[tokio::test]
async fn amend_updates_the_previous_commit() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit_all("first");
    let first = repo.git(&["rev-parse", "HEAD"]);
    repo.write("b.txt", "two\n");
    repo.git(&["add", "-A"]);

    let git = GitService::new();
    let result = git
        .commit(
            repo.path(),
            &GitCommitOptions {
                message: "second".to_string(),
                paths: None,
                amend: true,
            },
        )
        .await
        .expect("amend");
    assert_ne!(result.commit, first);
    assert_eq!(repo.git(&["log", "-1", "--pretty=%s"]), "second");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

#[tokio::test]
async fn checkout_creates_and_switches_branches() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit_all("init");

    let git = GitService::new();
    let created = git
        .checkout(
            repo.path(),
            &GitCheckoutOptions {
                branch: "feature/git-writes".to_string(),
                create: true,
            },
        )
        .await
        .expect("create branch");
    assert_eq!(created.branch, "feature/git-writes");
    assert_eq!(repo.current_branch(), "feature/git-writes");

    let base = repo.current_branch();
    repo.git(&["switch", "-q", base.as_str()]);
    let switched = git
        .checkout(
            repo.path(),
            &GitCheckoutOptions {
                branch: "feature/git-writes".to_string(),
                create: false,
            },
        )
        .await
        .expect("switch branch");
    assert_eq!(switched.branch, "feature/git-writes");
}

#[tokio::test]
async fn commit_with_nothing_staged_is_a_conflict() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit_all("init");

    let git = GitService::new();
    let error = git
        .commit(
            repo.path(),
            &GitCommitOptions {
                message: "noop".to_string(),
                paths: None,
                amend: false,
            },
        )
        .await
        .expect_err("nothing to commit");
    assert!(matches!(error, GitError::Conflict(_)), "got {error:?}");
}

#[tokio::test]
async fn invalid_inputs_are_rejected_before_git_runs() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit_all("init");

    let git = GitService::new();
    assert!(git
        .stage(repo.path(), &paths(&["../escape"]))
        .await
        .is_err());
    assert!(git
        .unstage(repo.path(), &paths(&["/etc/passwd"]))
        .await
        .is_err());
    assert!(git
        .commit(
            repo.path(),
            &GitCommitOptions {
                message: "   ".to_string(),
                paths: None,
                amend: false,
            },
        )
        .await
        .is_err());
    assert!(git
        .checkout(
            repo.path(),
            &GitCheckoutOptions {
                branch: "-x".to_string(),
                create: true,
            },
        )
        .await
        .is_err());
}

#[tokio::test]
async fn node_modules_paths_are_still_valid_paths() {
    // Only `.git` is special; any other name is accepted.
    require_git!();
    let repo = Repo::init();
    repo.write("node_modules/pkg/index.js", "module.exports = 1;\n");
    repo.write("a.txt", "one\n");
    repo.commit_all("init");
    repo.write("node_modules/pkg/index.js", "module.exports = 2;\n");

    let git = GitService::new();
    git.stage(repo.path(), &paths(&["node_modules/pkg/index.js"]))
        .await
        .expect("stage node_modules file");
    let status = repo.status_porcelain();
    assert!(status.contains("node_modules/pkg/index.js"));
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]),
        "node_modules/pkg/index.js"
    );
}

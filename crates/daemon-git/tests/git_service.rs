//! Integration tests against a real `git` binary.
//!
//! Each test is skipped gracefully when `git` is not installed.

use std::path::Path;
use std::process::{Command, Output};

use fleet_core_protocol::git::{DiffLineType, GitDiffOptions, GitFileStatus, GitLogRequest};
use fleet_daemon_git::{parse_porcelain_v2, parse_unified_diff, GitError, GitService};

/// Return whether a `git` executable is available.
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

/// A temporary repository with helpers for building test fixtures.
struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn init() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().expect("create temp dir"),
        };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test User"]);
        repo.git(&["config", "user.email", "test@example.com"]);
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

    fn git_fails(&self, args: &[&str]) -> bool {
        !self.output(args).status.success()
    }

    fn write(&self, relative: &str, content: &str) {
        self.write_bytes(relative, content.as_bytes());
    }

    fn write_bytes(&self, relative: &str, content: &[u8]) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, content).expect("write file");
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn current_branch(&self) -> String {
        self.git(&["rev-parse", "--abbrev-ref", "HEAD"])
    }
}

/// Build an empty log request (the protocol type has no `Default` impl).
fn log_request() -> GitLogRequest {
    GitLogRequest {
        limit: None,
        path: None,
        reference: None,
    }
}

/// Workspace-relative paths use `/`; tests only use simple names.
fn file_by_path<'a>(
    files: &'a [fleet_core_protocol::git::FileDiff],
    path: &str,
) -> &'a fleet_core_protocol::git::FileDiff {
    files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing diff for {path}: {files:?}"))
}

fn summary_by_path<'a>(
    files: &'a [fleet_core_protocol::git::DiffSummary],
    path: &str,
) -> &'a fleet_core_protocol::git::DiffSummary {
    files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing summary for {path}: {files:?}"))
}

#[tokio::test]
async fn status_reports_clean_modified_staged_deleted_and_untracked() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.write("b.txt", "two\n");
    repo.commit("initial");

    let service = GitService::new();

    let clean = service.status(repo.path()).await.expect("status");
    assert!(clean.is_clean);
    assert!(clean.files.is_empty());
    assert!(clean.branch.is_some());
    assert!(clean.head_oid.is_some());
    assert_eq!(clean.detached, Some(false));

    repo.write("a.txt", "one changed\n");
    let modified = service.status(repo.path()).await.expect("status");
    let change = modified
        .files
        .iter()
        .find(|file| file.path == "a.txt")
        .expect("a.txt change");
    assert_eq!(change.status, GitFileStatus::Modified);
    assert!(!change.staged);
    assert!(!modified.is_clean);

    repo.git(&["add", "a.txt"]);
    let staged = service.status(repo.path()).await.expect("status");
    let change = staged
        .files
        .iter()
        .find(|file| file.path == "a.txt")
        .expect("a.txt change");
    assert_eq!(change.status, GitFileStatus::Modified);
    assert!(change.staged);

    std::fs::remove_file(repo.path().join("b.txt")).expect("remove b.txt");
    let deleted = service.status(repo.path()).await.expect("status");
    let change = deleted
        .files
        .iter()
        .find(|file| file.path == "b.txt")
        .expect("b.txt change");
    assert_eq!(change.status, GitFileStatus::Deleted);
    assert!(!change.staged);

    repo.write("c.txt", "untracked\n");
    let untracked = service.status(repo.path()).await.expect("status");
    let change = untracked
        .files
        .iter()
        .find(|file| file.path == "c.txt")
        .expect("c.txt change");
    assert_eq!(change.status, GitFileStatus::Untracked);
    assert!(!change.staged);
}

#[tokio::test]
async fn status_detects_rename() {
    require_git!();
    let repo = Repo::init();
    repo.write("old name.txt", "content\n");
    repo.commit("initial");
    repo.git(&["mv", "old name.txt", "new name.txt"]);

    let status = GitService::new().status(repo.path()).await.expect("status");
    let renamed = status
        .files
        .iter()
        .find(|file| file.path == "new name.txt")
        .expect("renamed file");
    assert_eq!(renamed.status, GitFileStatus::Renamed);
    assert!(renamed.staged);
    assert_eq!(renamed.old_path.as_deref(), Some("old name.txt"));
}

#[tokio::test]
async fn status_detects_conflicts() {
    require_git!();
    let repo = Repo::init();
    repo.write("conflict.txt", "base\n");
    repo.commit("base");
    let base_branch = repo.current_branch();

    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("conflict.txt", "feature\n");
    repo.commit("feature change");

    repo.git(&["checkout", "-q", &base_branch]);
    repo.write("conflict.txt", "base branch\n");
    repo.commit("base change");

    assert!(repo.git_fails(&["merge", "feature"]));

    let status = GitService::new().status(repo.path()).await.expect("status");
    let conflicted = status
        .files
        .iter()
        .find(|file| file.path == "conflict.txt")
        .expect("conflicted file");
    assert_eq!(conflicted.status, GitFileStatus::Conflicted);
}

#[tokio::test]
async fn status_detects_detached_head() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.git(&["checkout", "-q", "--detach", "HEAD"]);

    let status = GitService::new().status(repo.path()).await.expect("status");
    assert!(status.branch.is_none());
    assert_eq!(status.detached, Some(true));
    assert!(status.head_oid.is_some());
}

#[tokio::test]
async fn discover_returns_none_for_non_repository() {
    require_git!();
    let dir = tempfile::tempdir().expect("temp dir");
    let service = GitService::new();

    let repository = service.discover(dir.path()).await.expect("discover");
    assert!(repository.is_none());

    let error = service.status(dir.path()).await.expect_err("status");
    assert!(matches!(error, GitError::NotARepository(_)));

    let error = service.branches(dir.path()).await.expect_err("branches");
    assert!(matches!(error, GitError::NotARepository(_)));

    let error = service
        .log(dir.path(), &log_request())
        .await
        .expect_err("log");
    assert!(matches!(error, GitError::NotARepository(_)));
}

#[tokio::test]
async fn discover_handles_linked_worktree() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit("initial");

    let worktree_dir = tempfile::tempdir().expect("worktree dir");
    let output = repo.output(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "worktree-branch",
        worktree_dir.path().to_str().expect("utf-8 path"),
    ]);
    if !output.status.success() {
        eprintln!(
            "skipping linked worktree assertion: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let path = worktree_dir.path();
    assert!(
        path.join(".git").is_file(),
        "linked worktrees use a .git file"
    );

    let repository = GitService::new()
        .discover(path)
        .await
        .expect("discover")
        .expect("inside a work tree");
    assert_eq!(repository.root, ".");
    assert!(repository.git_dir.contains("worktrees"));
    let canonical = std::fs::canonicalize(path).expect("canonical worktree");
    assert_eq!(repository.worktree.as_deref(), canonical.to_str());

    let status = GitService::new().status(path).await.expect("status");
    assert!(status.is_clean);
    assert_eq!(status.branch.as_deref(), Some("worktree-branch"));
}

#[tokio::test]
async fn diff_worktree_staged_path_filter_and_range() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "1\n2\n3\n4\n5\n");
    repo.write("b.txt", "b\n");
    repo.commit("initial");
    let first_commit = repo.git(&["rev-parse", "HEAD"]);
    let service = GitService::new();

    repo.write("a.txt", "1\n2 changed\n3\n4\n5\n");
    let diff = service
        .diff(repo.path(), &GitDiffOptions::default())
        .await
        .expect("worktree diff");
    let file = file_by_path(&diff.files, "a.txt");
    assert_eq!(file.status, GitFileStatus::Modified);
    assert_eq!(file.additions, 1);
    assert_eq!(file.deletions, 1);
    assert!(!file.is_binary);

    repo.git(&["add", "a.txt"]);
    let staged = service
        .diff(
            repo.path(),
            &GitDiffOptions {
                staged: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("staged diff");
    assert!(file_by_path(&staged.files, "a.txt").additions >= 1);

    let worktree_after_stage = service
        .diff(repo.path(), &GitDiffOptions::default())
        .await
        .expect("worktree diff");
    assert!(worktree_after_stage.files.is_empty());

    repo.write("b.txt", "b changed\n");
    let filtered = service
        .diff(
            repo.path(),
            &GitDiffOptions {
                path: Some("b.txt".into()),
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("filtered diff");
    assert_eq!(filtered.files.len(), 1);
    assert_eq!(filtered.files[0].path, "b.txt");

    repo.commit("second");
    let range = service
        .diff(
            repo.path(),
            &GitDiffOptions {
                base: Some(first_commit),
                head: Some("HEAD".into()),
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("range diff");
    assert!(!range.files.is_empty());
    assert!(range.files.iter().any(|file| file.path == "a.txt"));
}

#[tokio::test]
async fn diff_respects_context_lines() {
    require_git!();
    let repo = Repo::init();
    repo.write("lines.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
    repo.commit("initial");
    repo.write("lines.txt", "1\n2\n3\n4\nfive\n6\n7\n8\n9\n10\n");

    let diff = GitService::new()
        .diff(
            repo.path(),
            &GitDiffOptions {
                context_lines: Some(1),
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("diff");
    let file = file_by_path(&diff.files, "lines.txt");
    assert_eq!(file.hunks.len(), 1);
    assert_eq!(file.hunks[0].old_lines, 3);
    assert_eq!(file.hunks[0].new_lines, 3);
    assert_eq!(file.hunks[0].lines.len(), 4);
    assert_eq!(file.hunks[0].lines[0].line_type, DiffLineType::Context);
    assert_eq!(file.hunks[0].lines[1].line_type, DiffLineType::Deleted);
    assert_eq!(file.hunks[0].lines[2].line_type, DiffLineType::Added);
    assert_eq!(file.hunks[0].lines[3].line_type, DiffLineType::Context);
}

#[tokio::test]
async fn diff_includes_untracked_files() {
    require_git!();
    let repo = Repo::init();
    repo.write("tracked.txt", "tracked\n");
    repo.commit("initial");

    repo.write("new dir/untracked file.txt", "alpha\nbeta\n");
    let service = GitService::new();
    let diff = service
        .diff(
            repo.path(),
            &GitDiffOptions {
                include_untracked: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("diff");
    let file = file_by_path(&diff.files, "new dir/untracked file.txt");
    assert_eq!(file.status, GitFileStatus::Added);
    assert_eq!(file.additions, 2);
    assert_eq!(file.deletions, 0);
    assert!(!file.is_binary);
    assert_eq!(file.hunks.len(), 1);
    assert!(file.hunks[0]
        .lines
        .iter()
        .all(|line| line.line_type == DiffLineType::Added));

    let explicit = service
        .diff(
            repo.path(),
            &GitDiffOptions {
                path: Some("new dir/untracked file.txt".into()),
                include_untracked: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("explicit untracked path");
    assert_eq!(explicit.files.len(), 1);
    assert_eq!(explicit.files[0].status, GitFileStatus::Added);
}

#[tokio::test]
async fn diff_detects_binary_and_missing_newline() {
    require_git!();
    let repo = Repo::init();
    repo.write("no-newline.txt", "a\nb");
    repo.commit("initial");
    repo.write("no-newline.txt", "a\nB");

    let service = GitService::new();
    let diff = service
        .diff(repo.path(), &GitDiffOptions::default())
        .await
        .expect("diff");
    let file = file_by_path(&diff.files, "no-newline.txt");
    assert!(file
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .any(|line| line.line_type == DiffLineType::NoNewline));

    repo.write_bytes("binary.dat", &[0, 1, 2, 3, 0xff]);
    repo.git(&["add", "binary.dat"]);
    let staged = service
        .diff(
            repo.path(),
            &GitDiffOptions {
                staged: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("staged diff");
    assert!(staged.is_binary);
    let binary = file_by_path(&staged.files, "binary.dat");
    assert!(binary.is_binary);
    assert!(binary.hunks.is_empty());
    assert_eq!(binary.additions, 0);

    repo.write_bytes("untracked.bin", &[0, 9, 9]);
    let untracked = service
        .diff(
            repo.path(),
            &GitDiffOptions {
                include_untracked: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("diff with untracked");
    assert!(file_by_path(&untracked.files, "untracked.bin").is_binary);
}

#[tokio::test]
async fn diff_detects_rename() {
    require_git!();
    let repo = Repo::init();
    repo.write("old.txt", "content\n");
    repo.commit("initial");
    repo.git(&["mv", "old.txt", "new.txt"]);

    let diff = GitService::new()
        .diff(
            repo.path(),
            &GitDiffOptions {
                staged: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("diff");
    let file = file_by_path(&diff.files, "new.txt");
    assert_eq!(file.status, GitFileStatus::Renamed);
    assert_eq!(file.old_path.as_deref(), Some("old.txt"));
}

#[tokio::test]
async fn diff_summary_reports_staged_unstaged_and_untracked() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "1\n2\n");
    repo.write("b.txt", "1\n2\n");
    repo.commit("initial");

    repo.write("a.txt", "1\n2\n3\n");
    repo.write("b.txt", "1\n2\n3\n4\n");
    repo.git(&["add", "b.txt"]);
    repo.write("untracked.txt", "x\ny\n");

    let service = GitService::new();
    let unstaged = service
        .diff_summary(
            repo.path(),
            &GitDiffOptions {
                include_untracked: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("unstaged summary");
    let a = summary_by_path(&unstaged.files, "a.txt");
    assert!(!a.staged);
    assert_eq!(a.additions, 1);
    assert_eq!(a.deletions, 0);
    let untracked = summary_by_path(&unstaged.files, "untracked.txt");
    assert_eq!(untracked.status, GitFileStatus::Untracked);
    assert_eq!(untracked.additions, 2);
    assert!(unstaged.branch.is_some());

    let staged = service
        .diff_summary(
            repo.path(),
            &GitDiffOptions {
                staged: true,
                include_untracked: true,
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect("staged summary");
    let b = summary_by_path(&staged.files, "b.txt");
    assert!(b.staged);
    assert_eq!(b.additions, 2);
    assert!(staged.files.iter().all(|file| file.path != "a.txt"));
}

#[tokio::test]
async fn branches_and_log_return_expected_records() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit("first commit");
    let first_id = repo.git(&["rev-parse", "HEAD"]);
    repo.write("a.txt", "two\n");
    repo.commit("second commit");

    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);

    let service = GitService::new();
    let branches = service.branches(repo.path()).await.expect("branches");
    let current_name = repo.current_branch();
    let current = branches
        .iter()
        .find(|branch| branch.name == current_name)
        .expect("current branch");
    assert!(current.is_current);
    assert!(!current.is_remote);
    assert!(current.oid.is_some());

    let remote = branches
        .iter()
        .find(|branch| branch.name == "origin/main")
        .expect("remote branch");
    assert!(remote.is_remote);
    assert!(!remote.is_current);
    assert!(
        !branches.iter().any(|branch| branch.name == "origin/HEAD"),
        "remote HEAD symbolic ref must be skipped"
    );

    let entries = service.log(repo.path(), &log_request()).await.expect("log");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].message, "second commit");
    assert_eq!(entries[0].id.len(), 40);
    assert_eq!(entries[0].parents, vec![first_id]);
    assert_eq!(entries[1].message, "first commit");
    assert!(entries[1].parents.is_empty());

    let limited = service
        .log(
            repo.path(),
            &GitLogRequest {
                limit: Some(1),
                ..log_request()
            },
        )
        .await
        .expect("limited log");
    assert_eq!(limited.len(), 1);

    let by_path = service
        .log(
            repo.path(),
            &GitLogRequest {
                path: Some("a.txt".into()),
                ..log_request()
            },
        )
        .await
        .expect("log by path");
    assert_eq!(by_path.len(), 2);
}

#[tokio::test]
async fn log_rejects_invalid_reference_without_panicking() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit("initial");

    let error = GitService::new()
        .log(
            repo.path(),
            &GitLogRequest {
                reference: Some("does-not-exist".into()),
                ..log_request()
            },
        )
        .await
        .expect_err("missing reference must fail");
    assert!(matches!(error, GitError::InvalidPath(_)));

    let error = GitService::new()
        .log(
            repo.path(),
            &GitLogRequest {
                reference: Some("--output=/tmp/evil".into()),
                ..log_request()
            },
        )
        .await
        .expect_err("option-like reference must fail");
    assert!(matches!(error, GitError::InvalidPath(_)));
}

#[tokio::test]
async fn diff_rejects_escaping_paths() {
    require_git!();
    let repo = Repo::init();
    repo.write("a.txt", "one\n");
    repo.commit("initial");

    let error = GitService::new()
        .diff(
            repo.path(),
            &GitDiffOptions {
                path: Some("../../etc/passwd".into()),
                ..GitDiffOptions::default()
            },
        )
        .await
        .expect_err("escaping path must fail");
    assert!(matches!(error, GitError::InvalidPath(_)));
}

#[test]
fn porcelain_fixture_with_rename_and_spaces() {
    let mut bytes = Vec::new();
    for record in [
        "# branch.oid abc123",
        "# branch.head main",
        "1 .M N... 100644 100644 100644 aaa bbb file with space.txt",
        "2 R. N... 100644 100644 100644 aaa bbb R100 new name.txt",
        "old name.txt",
        "? untracked file.txt",
    ] {
        bytes.extend_from_slice(record.as_bytes());
        bytes.push(0);
    }

    let status = parse_porcelain_v2(&bytes).expect("parse");
    assert_eq!(status.files.len(), 3);
    assert_eq!(status.files[0].status, GitFileStatus::Modified);
    assert!(!status.files[0].staged);
    assert_eq!(status.files[1].status, GitFileStatus::Renamed);
    assert!(status.files[1].staged);
    assert_eq!(status.files[1].old_path.as_deref(), Some("old name.txt"));
    assert_eq!(status.files[2].path, "untracked file.txt");
}

#[test]
fn unified_diff_fixture_parses_quoted_paths_and_binary() {
    let diff = concat!(
        "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        "+++ \"b/caf\\303\\251.txt\"\n",
        "@@ -0,0 +1 @@\n",
        "+hello\n",
    );
    let file = parse_unified_diff("caf\u{e9}.txt", diff.as_bytes()).expect("parse");
    assert_eq!(file.path, "caf\u{e9}.txt");
    assert_eq!(file.status, GitFileStatus::Added);

    let binary = concat!(
        "diff --git a/logo.png b/logo.png\n",
        "Binary files a/logo.png and b/logo.png differ\n",
    );
    let file = parse_unified_diff("logo.png", binary.as_bytes()).expect("parse");
    assert!(file.is_binary);
    assert!(file.hunks.is_empty());
}

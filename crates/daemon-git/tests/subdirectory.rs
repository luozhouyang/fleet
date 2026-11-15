//! A workspace rooted at a repository subdirectory must never expose siblings.

use std::path::Path;
use std::process::{Command, Output};

use fleet_core_protocol::git::{GitDiffOptions, GitFileStatus, GitLogRequest};
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
            dir: tempfile::tempdir().expect("tempdir"),
        };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Subdir Test"]);
        repo.git(&["config", "user.email", "subdir@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        let output: Output = Command::new("git")
            .args(args)
            .current_dir(self.path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("run git");
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
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, content).expect("write");
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }
}

#[tokio::test]
async fn subdirectory_workspace_hides_sibling_files() {
    require_git!();
    let repo = Repo::init();
    repo.write("outside.txt", "outside\n");
    repo.write("sub/inside.txt", "inside\n");
    repo.commit_all("initial");

    repo.write("outside.txt", "outside changed\n");
    repo.write("sub/inside.txt", "inside changed\n");
    repo.write("sub/new.txt", "new\n");

    let service = GitService::new();
    let root = repo.path().join("sub");

    let status = service.status(&root).await.expect("status");
    let paths: Vec<&str> = status.files.iter().map(|file| file.path.as_str()).collect();
    assert!(paths.contains(&"inside.txt"), "paths: {paths:?}");
    assert!(paths.contains(&"new.txt"), "paths: {paths:?}");
    assert!(
        !paths.iter().any(|path| path.contains("outside")),
        "paths: {paths:?}"
    );

    let diff = service
        .diff(
            &root,
            &GitDiffOptions {
                include_untracked: true,
                ..Default::default()
            },
        )
        .await
        .expect("diff");
    let diff_paths: Vec<&str> = diff.files.iter().map(|file| file.path.as_str()).collect();
    assert!(diff_paths.contains(&"inside.txt"), "paths: {diff_paths:?}");
    assert!(diff_paths.contains(&"new.txt"), "paths: {diff_paths:?}");
    assert!(
        !diff_paths.iter().any(|path| path.contains("outside")),
        "paths: {diff_paths:?}"
    );
    assert!(diff
        .files
        .iter()
        .filter(|file| file.path == "inside.txt")
        .all(|file| file.status == GitFileStatus::Modified));

    let summary = service
        .diff_summary(
            &root,
            &GitDiffOptions {
                include_untracked: true,
                ..Default::default()
            },
        )
        .await
        .expect("summary");
    let summary_paths: Vec<&str> = summary
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert!(
        summary_paths.contains(&"inside.txt"),
        "paths: {summary_paths:?}"
    );
    assert!(
        !summary_paths.iter().any(|path| path.contains("outside")),
        "paths: {summary_paths:?}"
    );
}

#[tokio::test]
async fn revisions_cannot_be_smuggled_as_paths() {
    require_git!();
    let repo = Repo::init();
    repo.write("outside.txt", "outside\n");
    repo.write("sub/inside.txt", "inside\n");
    repo.commit_all("initial");
    let root = repo.path().join("sub");

    let service = GitService::new();
    for base in ["../outside.txt", ":(top)", "--output=/tmp/x"] {
        let error = service
            .diff(
                &root,
                &GitDiffOptions {
                    base: Some(base.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect_err("escaping revision must fail");
        assert!(
            matches!(error, GitError::InvalidPath(_)),
            "base {base}: {error:?}"
        );
    }

    let error = service
        .log(
            &root,
            &GitLogRequest {
                limit: None,
                path: Some(":(top)".to_string()),
                reference: None,
            },
        )
        .await
        .expect_err("pathspec magic must fail");
    assert!(matches!(error, GitError::InvalidPath(_)));
}

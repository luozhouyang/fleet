//! Temporary Git repositories used as workspaces.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// A throwaway repository with deterministic identity settings.
pub struct Repo {
    path: PathBuf,
}

impl Repo {
    /// Create `path` and initialize it with `initial` files committed.
    pub fn init(path: impl Into<PathBuf>, initial: &[(&str, &str)]) -> Result<Self> {
        let path = path.into();
        std::fs::create_dir_all(&path)
            .with_context(|| format!("create repository {}", path.display()))?;
        let repo = Self { path };
        repo.run(["init", "-q"])?;
        repo.run(["config", "user.name", "Fleet E2E"])?;
        repo.run(["config", "user.email", "e2e@example.com"])?;
        repo.run(["config", "commit.gpgsign", "false"])?;
        for (relative, content) in initial {
            repo.write(relative, content)?;
        }
        repo.add_all()?;
        repo.commit("initial")?;
        Ok(repo)
    }

    /// Create the repository but leave it empty (no initial commit).
    pub fn init_empty(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        std::fs::create_dir_all(&path)
            .with_context(|| format!("create repository {}", path.display()))?;
        let repo = Self { path };
        repo.run(["init", "-q"])?;
        repo.run(["config", "user.name", "Fleet E2E"])?;
        repo.run(["config", "user.email", "e2e@example.com"])?;
        repo.run(["config", "commit.gpgsign", "false"])?;
        Ok(repo)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_string(&self) -> String {
        self.path.display().to_string()
    }

    pub fn write(&self, relative: &str, content: &str) -> Result<()> {
        let target = self.path.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content).with_context(|| format!("write {}", target.display()))
    }

    pub fn add_all(&self) -> Result<()> {
        self.run(["add", "-A"]).map(|_| ())
    }

    pub fn commit(&self, message: &str) -> Result<()> {
        self.run(["commit", "-qm", message]).map(|_| ())
    }

    pub fn run<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(args)
            .output()
            .with_context(|| format!("run git in {}", self.path.display()))?;
        if !output.status.success() {
            bail!(
                "git {:?} failed ({})\nstdout:\n{}\nstderr:\n{}",
                args,
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub fn head(&self) -> Result<String> {
        self.run(["rev-parse", "HEAD"])
    }

    pub fn branch(&self) -> Result<String> {
        self.run(["rev-parse", "--abbrev-ref", "HEAD"])
    }

    pub fn staged_names(&self) -> Result<Vec<String>> {
        Ok(self
            .run(["diff", "--cached", "--name-only"])?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    pub fn last_subject(&self) -> Result<String> {
        self.run(["log", "-1", "--pretty=%s"])
    }

    pub fn show_stat(&self) -> Result<String> {
        self.run(["show", "--stat", "--oneline", "HEAD"])
    }
}

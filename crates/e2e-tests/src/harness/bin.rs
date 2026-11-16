//! Locating the workspace binaries without re-entering cargo.
//!
//! E2E tests are `#[ignore]`d and run through `pnpm e2e`, which builds the
//! binaries first. Invoking cargo from inside a test risks contending on the
//! target-directory lock, so the harness only searches for already-built
//! artifacts and reports a clear error when they are missing.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// Paths to the three binaries under test.
#[derive(Debug, Clone)]
pub struct Bins {
    pub admin: PathBuf,
    pub daemon: PathBuf,
    pub fleet: PathBuf,
}

/// Workspace root, derived from this crate's manifest directory.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives under <workspace>/crates")
        .to_path_buf()
}

impl Bins {
    pub fn locate() -> Result<Self> {
        if let Some(dir) = std::env::var_os("FLEET_E2E_BIN_DIR") {
            return Self::from_dir(Path::new(&dir));
        }

        let root = workspace_root();
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));

        let profile = std::env::var("FLEET_E2E_PROFILE").ok();
        let mut candidates = Vec::new();
        if let Some(profile) = profile {
            candidates.push(target.join(profile));
        }
        candidates.push(target.join("debug"));
        candidates.push(target.join("release"));
        if let Ok(entries) = std::fs::read_dir(&target) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    candidates.push(path.join("debug"));
                    candidates.push(path.join("release"));
                }
            }
        }

        for candidate in &candidates {
            if Self::exists_in(candidate) {
                return Self::from_dir(candidate);
            }
        }

        bail!(
            "fleet E2E binaries were not found under {}. Build them first:\n  \
             cargo build -p fleet-admin -p fleet-daemon -p fleet-cli\n\
             or run `pnpm e2e`. Searched: {}",
            target.display(),
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    fn from_dir(dir: &Path) -> Result<Self> {
        let bins = Self {
            admin: executable(dir, "fleet-admin"),
            daemon: executable(dir, "fleet-daemon"),
            fleet: executable(dir, "fleet"),
        };
        for (name, path) in [
            ("fleet-admin", &bins.admin),
            ("fleet-daemon", &bins.daemon),
            ("fleet", &bins.fleet),
        ] {
            if !path.is_file() {
                bail!(
                    "{name} is missing at {}. Run `pnpm e2e` or \
                     `cargo build -p fleet-admin -p fleet-daemon -p fleet-cli`.",
                    path.display()
                );
            }
        }
        Ok(bins)
    }

    fn exists_in(dir: &Path) -> bool {
        ["fleet-admin", "fleet-daemon", "fleet"]
            .iter()
            .all(|name| executable(dir, name).is_file())
    }
}

fn executable(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join(name)
    }
}

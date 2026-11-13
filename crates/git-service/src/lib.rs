//! Git access for Agent Fleet.
//!
//! Read operations are the historical core; write operations (stage, unstage,
//! commit, checkout) validate every user-provided path and branch name first.
//!
//! This crate is the only component allowed to run `git`. It wraps the Git
//! CLI with an argument-vector based command runner, bounded output and
//! timeouts, and parses the stable machine-readable formats:
//!
//! - `git status --porcelain=v2 -z --branch`
//! - `git diff --unified=N -z` / `--numstat` / `--name-status`
//! - `git for-each-ref`
//! - `git log --format=...`
//!
//! Untracked files are read from disk with root-bound checks and synthesized
//! as added-file diffs, because `git diff` does not include them.

#![warn(missing_docs)]

mod branches;
mod command;
mod diff;
mod diff_parser;
mod discovery;
mod error;
mod log;
mod service;
mod status;
mod summary;
mod write;

pub use diff_parser::parse_unified_diff;
pub use error::{GitError, Result};
pub use service::{
    GitLimits, GitService, DEFAULT_DIFF_TIMEOUT, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_STATUS_TIMEOUT,
};
pub use status::parse_porcelain_v2;

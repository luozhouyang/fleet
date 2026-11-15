//! Child process management for Agent Fleet.
//!
//! This crate provides two process models:
//!
//! - [`ManagedProcess`]: a long-lived child with piped stdin/stdout/stderr,
//!   broadcast output events, and process-group signalling.
//! - [`PtyProcess`]: a child attached to a pseudo-terminal for interactive
//!   terminal-compatible agents.
//!
//! It also provides [`run_capture`] for short-lived commands (used by the Git
//! service) with a timeout and bounded output buffers.
//!
//! On Unix children are placed in their own process group so that terminating
//! the process also terminates its descendants, and [`ManagedProcess`] kills
//! the group on drop to avoid orphaned agents.

#![warn(missing_docs)]

mod command;
mod error;
mod output;
mod process;
mod pty;
mod signal;

pub use command::CommandSpec;
pub use error::{ProcessError, ProcessResult};
pub use output::{run_capture, CapturedOutput, ProcessEvent};
pub use process::ManagedProcess;
pub use pty::PtyProcess;

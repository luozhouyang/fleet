//! Black-box end-to-end tests for the fleet control plane and daemons.
//!
//! The tests build (or expect) the `fleet-admin`, `fleet-daemon` and `fleet`
//! binaries, start them on ephemeral loopback ports with isolated temporary
//! state, and drive them over HTTP and the CLI. They deliberately use no
//! containers and no fixed ports, so the suite runs on the host and tests can
//! execute concurrently.

pub mod api;
pub mod harness;

pub use api::Api;
pub use harness::{
    env::TestEnv, free_port, git::Repo, process::ManagedProcess, wait::wait_until, Admin,
    AdminOptions, Daemon, DaemonOptions,
};

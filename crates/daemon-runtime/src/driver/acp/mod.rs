//! Generic ACP driver: process, connection, negotiation, mappers.
//!
//! The driver targets ACP v1 (stable) and v2 (draft, behind the `acp_v2`
//! feature flag). Claude Code ACP and any other ACP-compatible agent share
//! this implementation; per-agent behavior comes from the agent definition.

pub mod capabilities;
pub mod client;
pub mod config;
pub mod connection;
pub mod driver;
pub mod mapper;
pub mod permissions;
pub mod process;
pub mod session;
pub mod terminal;

#[cfg(test)]
pub(crate) mod tests;

pub use config::{AcpDriverConfig, AcpProtocolPolicy, AcpProtocolVersion};
pub use driver::AcpDriver;

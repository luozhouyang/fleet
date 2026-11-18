//! Agent Client Protocol (ACP) provider.
//!
//! [`adapter::AcpProvider`] launches an external ACP agent over stdio, drives
//! the v1 handshake and session lifecycle, and maps ACP session updates,
//! permission requests and form elicitations into the unified agent runtime.
//! [`mapper`] contains the pure mapping functions; [`protocol`] re-exports the
//! ACP schema types the adapter uses.

pub mod adapter;
pub mod protocol;

/// Backwards-compatible path for the v1 mapper.
pub use crate::driver::acp::mapper::v1 as mapper;

//! Agent Client Protocol (ACP) provider.
//!
//! [`adapter::AcpProvider`] launches an external ACP agent over stdio, drives
//! the v1 handshake and session lifecycle, and maps ACP session updates,
//! permission requests and form elicitations into the unified agent runtime.
//! [`mapper`] contains the pure mapping functions; [`protocol`] re-exports the
//! ACP schema types the adapter uses.

pub mod adapter;
pub mod mapper;
pub mod protocol;

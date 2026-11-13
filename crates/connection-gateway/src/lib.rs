//! Reverse WebSocket connection between the machine daemon and the control plane.
//!
//! The server side (`ConnectionGateway`) accepts machine WebSockets, authenticates
//! them with an Ed25519 challenge-response, tracks live connections and routes
//! request/response frames. The client side (`MachineConnectionClient`) performs
//! the mirror image and reconnects with exponential backoff.

mod client;
mod error;
mod registry;
mod server;

pub use client::*;
pub use error::*;
pub use registry::*;
pub use server::*;

pub use agent_fleet_protocol::connection::ConnectionState;

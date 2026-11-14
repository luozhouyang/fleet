//! In-memory machine registry backed by the shared SQLite storage.
//!
//! The registry tracks machines known to the agent admin. Live WebSocket
//! connections are tracked separately by `agent-fleet-connection-gateway`; this
//! crate only owns persistent machine metadata and last-seen state.

mod error;
mod registry;

pub use error::*;
pub use registry::*;

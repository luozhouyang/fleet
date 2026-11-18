//! Pure ACP protocol to Fleet event mappings.
//!
//! The v1 mapper is shared with the legacy `AcpProvider` adapter. The v2
//! mapper implements upsert semantics, foreground activity tracking and
//! prompt correlation for the draft v2 protocol.

pub mod common;
pub mod messages;
pub mod state;
pub mod tools;
pub mod v1;
pub mod v2;

pub use state::AcpUpdateState;

//! Reverse-connection protocol between the machine daemon and the agent admin.
//!
//! Wire format is WebSocket text frames carrying JSON [`ConnectionEnvelope`]s.
//! Large payloads are split into [`StreamFrame`]s (base64-encoded chunks).

mod auth;
mod envelope;
mod event;
mod request;
mod response;
mod stream;

pub use auth::*;
pub use envelope::*;
pub use event::*;
pub use request::*;
pub use response::*;
pub use stream::*;

/// Well-known event names pushed over the connection and forwarded to browsers.
pub mod events {
    pub const AGENT_EVENT: &str = "agent.event";
    pub const WORKSPACE_EVENT: &str = "workspace.event";
    pub const GIT_STATUS_CHANGED: &str = "git.status.changed";
    pub const MACHINE_STATUS: &str = "machine.status";
    pub const CONNECTION_STATE: &str = "connection.state";
    pub const AGENT_SESSION: &str = "agent.session";
}

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

/// Capabilities negotiated in `connection.hello` and echoed in
/// `connection.accepted`.
///
/// Capabilities are additive: both sides may ignore an unknown value. The
/// admin only advertises what it actually implements, and the daemon should
/// only advertise features it will honor for the whole connection.
pub mod capabilities {
    /// Durable event delivery with outbox cursors and `events.ack`.
    pub const DURABLE_EVENTS: &str = "durable_events";
    /// `events.ack` accepts a batch of applied outbox sequences in `cursors`.
    pub const ACK_BATCH: &str = "ack_batch";
    /// `RequestOptions.idempotency_key` is honored for mutating RPCs.
    pub const IDEMPOTENCY: &str = "idempotency";
    /// The daemon persists `connection.rotate_token` requests.
    pub const TOKEN_ROTATION: &str = "token_rotation";

    /// Capabilities implemented by this admin.
    pub const SUPPORTED_BY_ADMIN: &[&str] =
        &[DURABLE_EVENTS, ACK_BATCH, IDEMPOTENCY, TOKEN_ROTATION];

    /// Capabilities implemented by this daemon.
    pub const SUPPORTED_BY_DAEMON: &[&str] =
        &[DURABLE_EVENTS, ACK_BATCH, IDEMPOTENCY, TOKEN_ROTATION];
}

/// Well-known event names pushed over the connection and forwarded to browsers.
pub mod events {
    pub const AGENT_EVENT: &str = "agent.event";
    pub const WORKSPACE_EVENT: &str = "workspace.event";
    pub const GIT_STATUS_CHANGED: &str = "git.status.changed";
    pub const MACHINE_STATUS: &str = "machine.status";
    pub const CONNECTION_STATE: &str = "connection.state";
    pub const AGENT_SESSION: &str = "agent.session";
}

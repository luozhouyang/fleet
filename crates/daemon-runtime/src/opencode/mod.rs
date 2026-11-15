//! OpenCode server adapter.
//!
//! Integrates with the OpenCode v2 HTTP API (`opencode serve`): sessions are
//! created over HTTP and streamed over the server-sent-events endpoint
//! `/api/event`. The adapter spawns a private server on a random loopback port
//! unless `OPENCODE_SERVER_URL` points at an existing one.

mod adapter;
mod client;
mod mapper;
mod server;

pub use adapter::OpenCodeProvider;
pub use client::{NativeSessionInfo, OpenCodeClient};
pub use mapper::{map_event, MappedEvent, ParsedEvent};
pub use server::OpenCodeServer;

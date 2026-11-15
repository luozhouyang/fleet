//! In-process publish/subscribe bus for platform events.
//!
//! [`EventBus`] wraps a Tokio broadcast channel. It is cheap to clone and is
//! shared between the agent-admin connection gateway, the agent runtime and
//! the browser event WebSocket. Publishing never blocks; slow subscribers may
//! lag and are handled independently by [`Subscription`].

#![warn(missing_docs)]

mod bus;
mod filter;

pub use bus::{EventBus, DEFAULT_CAPACITY};
pub use filter::{EventFilter, Subscription};

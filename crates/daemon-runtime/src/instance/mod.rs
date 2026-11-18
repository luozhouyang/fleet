//! Agent instance lifecycle.
//!
//! An instance is one actual agent process or remote connection. It is
//! separate from the Fleet session: in this version one ACP instance serves
//! exactly one session, while native drivers keep their existing process
//! model. The instance manager indexes instances by id and by session.

pub mod manager;
pub mod state;

pub use manager::InstanceManager;
pub use state::{AgentInstance, InstanceStatus};

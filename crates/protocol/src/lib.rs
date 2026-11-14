//! Unified protocol types shared by the Agent Daemon, the Control Plane and the Web UI.
//!
//! This crate is the stable contract of the platform. It must not depend on any
//! business implementation crate. All cross-module data exchange goes through the
//! types defined here.

pub mod agent;
pub mod audit;
pub mod authz;
pub mod common;
pub mod connection;
pub mod git;
pub mod machine;
pub mod rpc;
pub mod workspace;

/// Version of the connection protocol spoken between daemon and agent admin.
pub const PROTOCOL_VERSION: u16 = 1;

pub use common::{
    new_id, now, Ack, ApiError, EventId, MachineId, MessageId, OrganizationId, Page, ProjectId,
    RequestId, SessionId, StreamId, Timestamp, TurnId, UserId, WorkspaceId,
};

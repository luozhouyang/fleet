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
pub mod pairing;
pub mod rpc;
pub mod workspace;

/// Version of the connection protocol spoken between daemon and agent admin.
pub const PROTOCOL_VERSION: u16 = 1;

/// Oldest daemon protocol version this admin accepts.
///
/// The window is N−2: a daemon upgrade may lag a server upgrade by two
/// released protocol versions. Bump this together with [`PROTOCOL_VERSION`].
pub const MIN_SUPPORTED_PROTOCOL_VERSION: u16 = if PROTOCOL_VERSION > 2 {
    PROTOCOL_VERSION - 2
} else {
    1
};

/// Whether `version` falls inside the supported N−2 window.
pub const fn is_supported_protocol_version(version: u16) -> bool {
    version >= MIN_SUPPORTED_PROTOCOL_VERSION && version <= PROTOCOL_VERSION
}

pub use common::{
    new_id, now, Ack, ApiError, EventId, MachineId, MessageId, OrganizationId, Page, ProjectId,
    RequestId, SessionId, StreamId, Timestamp, TurnId, UserId, WorkspaceId,
};

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn protocol_window_is_current_plus_two_previous_versions() {
        assert!(is_supported_protocol_version(PROTOCOL_VERSION));
        assert!(is_supported_protocol_version(
            MIN_SUPPORTED_PROTOCOL_VERSION
        ));
        assert!(!is_supported_protocol_version(PROTOCOL_VERSION + 1));
        if MIN_SUPPORTED_PROTOCOL_VERSION > 1 {
            assert!(!is_supported_protocol_version(
                MIN_SUPPORTED_PROTOCOL_VERSION - 1
            ));
        }
    }
}

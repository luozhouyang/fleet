use serde::{Deserialize, Serialize};

use crate::common::{MachineId, Timestamp};
use crate::machine::MachineStatus;

/// First frame sent by the daemon after the WebSocket is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloPayload {
    pub machine_id: MachineId,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    /// Base64-encoded Ed25519 public key.
    pub public_key: String,
}

/// Server challenge; the daemon signs the nonce with its private key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub challenge_id: String,
    /// Base64-encoded random nonce.
    pub nonce: String,
    pub issued_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// Proof that the owner of the previously pinned key authorized a new key.
///
/// The old key signs [`key_rotation_statement`], which binds the machine id,
/// the new public key and the challenge nonce. This makes every rotation proof
/// single-use and prevents replaying it in a later connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyRotationProof {
    /// Base64-encoded new Ed25519 public key (must equal `hello.public_key`).
    pub new_public_key: String,
    /// Base64-encoded signature by the previously pinned key.
    pub old_signature: String,
}

/// Daemon answer to a challenge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub machine_id: MachineId,
    pub challenge_id: String,
    /// Base64-encoded Ed25519 signature over the nonce bytes.
    pub signature: String,
    /// Present when the machine rotated its key since the last connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<KeyRotationProof>,
}

/// Canonical statement signed by the old key during rotation.
///
/// The exact byte string is part of the protocol: both sides must use this
/// function. `nonce` is the base64 nonce from [`AuthChallenge`].
pub fn key_rotation_statement(machine_id: &str, new_public_key: &str, nonce: &str) -> String {
    format!(
        "agent-fleet-key-rotation-v1\nmachine_id={machine_id}\nnew_public_key={new_public_key}\nnonce={nonce}"
    )
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    #[test]
    fn statement_is_canonical_and_input_sensitive() {
        let statement = key_rotation_statement("m_1", "pub", "nonce");
        assert_eq!(
            statement,
            "agent-fleet-key-rotation-v1\nmachine_id=m_1\nnew_public_key=pub\nnonce=nonce"
        );
        assert_ne!(statement, key_rotation_statement("m_2", "pub", "nonce"));
        assert_ne!(statement, key_rotation_statement("m_1", "pub2", "nonce"));
        assert_ne!(statement, key_rotation_statement("m_1", "pub", "nonce2"));
    }
}

/// Sent by the control plane after successful authentication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionAccepted {
    pub connection_id: String,
    pub machine_id: MachineId,
    pub server_version: String,
    pub protocol_version: u16,
    pub heartbeat_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisconnectPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Client-side connection state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Authenticating,
    Connected,
    Reconnecting,
    Failed,
}

/// Machine status event pushed by the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineStatusEvent {
    pub machine_id: MachineId,
    pub status: MachineStatus,
    pub timestamp: Timestamp,
}

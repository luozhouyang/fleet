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

/// Daemon answer to a challenge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub machine_id: MachineId,
    pub challenge_id: String,
    /// Base64-encoded Ed25519 signature over the nonce bytes.
    pub signature: String,
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

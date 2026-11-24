use std::fmt;

use serde::{Deserialize, Serialize};

use crate::common::{MachineId, Timestamp};
use crate::machine::MachineStatus;

/// First frame sent by the daemon after the WebSocket is open.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
    /// Optional feature names the daemon implements. Unknown names are
    /// ignored, so older daemons that omit the field stay compatible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

impl fmt::Debug for HelloPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HelloPayload")
            .field("machine_id", &self.machine_id)
            .field("name", &self.name)
            .field("hostname", &self.hostname)
            .field("os", &self.os)
            .field("arch", &self.arch)
            .field("daemon_version", &self.daemon_version)
            .field("protocol_version", &self.protocol_version)
            .field("public_key", &self.public_key)
            .field(
                "pairing_code",
                &self.pairing_code.as_ref().map(|_| "<redacted>"),
            )
            .field("capabilities", &self.capabilities)
            .finish()
    }
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

/// Sent by the agent admin after successful authentication.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionAccepted {
    pub connection_id: String,
    pub machine_id: MachineId,
    pub server_version: String,
    pub protocol_version: u16,
    pub heartbeat_interval_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// Intersection of the daemon's advertised capabilities and the admin's.
    /// Older daemons ignore the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

impl fmt::Debug for ConnectionAccepted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionAccepted")
            .field("connection_id", &self.connection_id)
            .field("machine_id", &self.machine_id)
            .field("server_version", &self.server_version)
            .field("protocol_version", &self.protocol_version)
            .field("heartbeat_interval_ms", &self.heartbeat_interval_ms)
            .field(
                "machine_token",
                &self.machine_token.as_ref().map(|_| "<redacted>"),
            )
            .field("org_id", &self.org_id)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisconnectPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Server-directed machine credential rotation.
///
/// Sent as a request over an authenticated machine connection. The daemon must
/// persist the new token before answering, so a successful response means the
/// credential survives a daemon restart.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RotateTokenPayload {
    /// Replacement machine token (`flm_...`).
    pub machine_token: String,
}

impl fmt::Debug for RotateTokenPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RotateTokenPayload")
            .field("machine_token", &"<redacted>")
            .finish()
    }
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

#[cfg(test)]
mod redaction_tests {
    use super::*;

    #[test]
    fn hello_and_accepted_redact_secrets() {
        let hello = HelloPayload {
            machine_id: "m_1".into(),
            name: "test".into(),
            hostname: "test.local".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            daemon_version: "0.1.0".into(),
            protocol_version: 1,
            public_key: "public".into(),
            pairing_code: Some("ABCD-EFGH-JKMN".into()),
            capabilities: vec!["token_rotation".into()],
        };
        let rendered = format!("{hello:?}");
        assert!(!rendered.contains("ABCD-EFGH-JKMN"));
        assert!(rendered.contains("<redacted>"));

        let accepted = ConnectionAccepted {
            connection_id: "conn_1".into(),
            machine_id: "m_1".into(),
            server_version: "0.1.0".into(),
            protocol_version: 1,
            heartbeat_interval_ms: 15_000,
            machine_token: Some("flm_secret".into()),
            org_id: Some("org_default".into()),
            capabilities: vec!["token_rotation".into()],
        };
        let rendered = format!("{accepted:?}");
        assert!(!rendered.contains("flm_secret"));
        assert!(rendered.contains("<redacted>"));
    }
}

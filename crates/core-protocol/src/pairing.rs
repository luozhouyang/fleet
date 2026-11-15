//! Pairing-code and per-machine-token wire types.
//!
//! These types live in the protocol crate so `fleet-core-connection` and
//! `fleet-admin-storage` can share them without a dependency cycle.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::common::Timestamp;

/// `POST /api/v1/pairing/codes` request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CreatePairingCodeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
}

/// `POST /api/v1/pairing/codes` response. The plain text code is returned once.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingCodeResponse {
    pub id: String,
    pub code: String,
    pub expires_at: Timestamp,
    pub org_id: String,
}

impl fmt::Debug for PairingCodeResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingCodeResponse")
            .field("id", &self.id)
            .field("code", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("org_id", &self.org_id)
            .finish()
    }
}

/// What the gateway knows about a machine before authenticating it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineAuthState {
    pub pinned_key: Option<String>,
}

/// A live pairing code, after validation.
#[derive(Debug, Clone, PartialEq)]
pub struct PairingCodeInfo {
    pub id: String,
    pub org_id: String,
    pub created_by: String,
    pub machine_name_hint: Option<String>,
    pub expires_at: Timestamp,
    pub attempts_left: u32,
}

/// Internal validation result. Every non-`Valid` status is rendered to the
/// client as `pairing_code_invalid` to prevent enumeration.
#[derive(Debug, Clone, PartialEq)]
pub enum PairingCodeStatus {
    Valid(PairingCodeInfo),
    Invalid,
    Expired { id: String, org_id: String },
    Used { id: String, org_id: String },
    Locked { id: String, org_id: String },
}

/// Input for the atomic pairing transaction.
#[derive(Debug, Clone)]
pub struct PairingCompletion {
    pub code_id: String,
    pub machine_id: String,
    pub public_key: String,
    pub token_hash: String,
    pub machine_name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub rotation_verified: bool,
}

/// Result of a successful pairing transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct PairingOutcome {
    pub machine_id: String,
    pub org_id: String,
    pub paired_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::now;

    #[test]
    fn pairing_code_response_redacts_the_code() {
        let response = PairingCodeResponse {
            id: "pair_1".into(),
            code: "ABCD-EFGH-JKMN".into(),
            expires_at: now(),
            org_id: "org_default".into(),
        };
        let rendered = format!("{response:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("ABCD-EFGH-JKMN"));
    }

    #[test]
    fn pairing_status_roundtrips_through_serde() {
        let request = CreatePairingCodeRequest {
            org_id: Some("org_default".into()),
            machine_name: Some("workstation".into()),
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: CreatePairingCodeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(request, back);
    }
}

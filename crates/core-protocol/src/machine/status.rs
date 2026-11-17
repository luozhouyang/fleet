use serde::{Deserialize, Serialize};

use super::MachineStatus;
use crate::common::Timestamp;

/// Periodic liveness payload sent by the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatPayload {
    pub sent_at: Timestamp,
    pub sequence: u64,
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub active_sessions: u32,
    #[serde(default)]
    pub status: Option<MachineStatus>,
}

/// Response to a heartbeat, used for clock/latency observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatAckPayload {
    pub received_at: Timestamp,
    pub sequence: u64,
    #[serde(default)]
    pub server_time: Option<Timestamp>,
}

/// Health/version information exposed by the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthPayload {
    pub machine_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub uptime_secs: u64,
    pub active_sessions: u32,
    pub timestamp: Timestamp,
}

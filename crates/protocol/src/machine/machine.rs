use serde::{Deserialize, Serialize};

use crate::agent::ProviderInfo;
use crate::common::{MachineId, Timestamp};
use crate::workspace::WorkspaceSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineStatus {
    Online,
    Offline,
    Degraded,
    Unknown,
}

impl MachineStatus {
    pub fn is_online(self) -> bool {
        matches!(self, Self::Online | Self::Degraded)
    }
}

/// A machine registered with the agent admin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub id: MachineId,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub status: MachineStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceSummary>,
    #[serde(default)]
    pub workspace_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

/// Payload sent by a daemon during `connection.hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineRegistration {
    pub machine_id: MachineId,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub public_key: String,
    #[serde(default)]
    pub providers: Vec<ProviderInfo>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSummary>,
}

//! Audit-log contracts for privileged operations (currently Git writes).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::common::Timestamp;

/// One recorded privileged action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Monotonic database row id.
    pub id: i64,
    /// Who performed the action.
    pub actor: String,
    /// Stable action name, e.g. `git.commit`.
    pub action: String,
    /// Entity the action targeted, usually a workspace id.
    pub subject: String,
    /// Structured context (paths, branch, outcome).
    #[serde(default)]
    pub details: Value,
    /// When the record was written.
    pub created_at: Timestamp,
}

/// Response for the audit-log listing endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditLogPage {
    /// Entries ordered newest first.
    pub entries: Vec<AuditLogEntry>,
}

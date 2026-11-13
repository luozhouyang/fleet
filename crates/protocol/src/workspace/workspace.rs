use serde::{Deserialize, Serialize};

use crate::common::{MachineId, ProjectId, Timestamp, WorkspaceId};

/// A registered workspace root on a machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub machine_id: MachineId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    pub name: String,
    /// Absolute path on the machine. Never used by the browser as a path input.
    pub root_path: String,
    pub is_git_repository: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Lightweight workspace descriptor embedded in machine descriptions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    pub name: String,
    pub root_path: String,
    pub is_git_repository: bool,
}

impl From<&Workspace> for WorkspaceSummary {
    fn from(ws: &Workspace) -> Self {
        Self {
            id: ws.id.clone(),
            name: ws.name.clone(),
            root_path: ws.root_path.clone(),
            is_git_repository: ws.is_git_repository,
        }
    }
}

/// Request payload for `workspace.register`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterWorkspaceRequest {
    /// Absolute path to an existing directory on the machine.
    pub root_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
}

//! Row mappings for admin storage.

use fleet_core_common::ApiError;
use fleet_core_protocol::agent::{AgentSession, ProviderInfo};
use fleet_core_protocol::authz::{Organization, OrganizationKind, User};
use fleet_core_protocol::machine::Machine;
use fleet_core_protocol::workspace::{Workspace, WorkspaceSummary};
use sqlx::FromRow;

use crate::helpers::*;

#[derive(Debug, FromRow)]
pub(crate) struct WorkspaceRow {
    id: String,
    machine_id: String,
    project_id: Option<String>,
    name: String,
    root_path: String,
    is_git_repository: i64,
    created_at: String,
    updated_at: String,
}

impl WorkspaceRow {
    pub(crate) fn into_workspace(self) -> Workspace {
        Workspace {
            id: self.id,
            machine_id: self.machine_id,
            project_id: self.project_id,
            name: self.name,
            root_path: self.root_path,
            is_git_repository: self.is_git_repository != 0,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct SessionRow {
    id: String,
    machine_id: String,
    workspace_id: Option<String>,
    provider: String,
    agent_type: String,
    status: String,
    cwd: String,
    title: Option<String>,
    last_error_json: Option<String>,
    metadata_json: String,
    created_at: String,
    updated_at: String,
}

impl SessionRow {
    pub(crate) fn into_session(self) -> AgentSession {
        AgentSession {
            id: self.id,
            machine_id: self.machine_id,
            workspace_id: self.workspace_id,
            provider: self.provider,
            agent_type: self.agent_type,
            status: parse_session_status(&self.status),
            cwd: self.cwd,
            title: self.title,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
            last_error: decode_opt_json::<ApiError>(self.last_error_json),
            metadata: decode_json_value(&self.metadata_json),
            ui: None,
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct OrganizationRow {
    id: String,
    name: String,
    kind: String,
    owner_user_id: Option<String>,
    deleted_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl TryFrom<OrganizationRow> for Organization {
    type Error = ();

    fn try_from(row: OrganizationRow) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            name: row.name,
            kind: OrganizationKind::parse(&row.kind).ok_or(())?,
            owner_user_id: row.owner_user_id,
            deleted_at: parse_opt_ts(row.deleted_at),
            created_at: parse_ts(&row.created_at),
            updated_at: parse_ts(&row.updated_at),
        })
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct UserRow {
    id: String,
    email: Option<String>,
    name: String,
    avatar_url: Option<String>,
    created_at: String,
    updated_at: String,
    token_issued_at: Option<String>,
    token_expires_at: Option<String>,
    disabled_at: Option<String>,
}

impl TryFrom<UserRow> for User {
    type Error = ();

    fn try_from(row: UserRow) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            email: row.email,
            name: row.name,
            avatar_url: row.avatar_url,
            created_at: parse_ts(&row.created_at),
            updated_at: parse_ts(&row.updated_at),
            token_issued_at: parse_opt_ts(row.token_issued_at),
            token_expires_at: parse_opt_ts(row.token_expires_at),
            disabled_at: parse_opt_ts(row.disabled_at),
        })
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct MachineRow {
    id: String,
    name: String,
    hostname: String,
    os: String,
    arch: String,
    daemon_version: String,
    protocol_version: i64,
    status: String,
    public_key: Option<String>,
    connected_at: Option<String>,
    last_seen_at: Option<String>,
    providers_json: String,
    workspaces_json: String,
    created_at: String,
    updated_at: String,
}

impl MachineRow {
    pub(crate) fn into_machine(self) -> Machine {
        let providers = decode_json_or_default::<Vec<ProviderInfo>>(&self.providers_json);
        let workspaces = decode_json_or_default::<Vec<WorkspaceSummary>>(&self.workspaces_json);

        Machine {
            id: self.id,
            name: self.name,
            hostname: self.hostname,
            os: self.os,
            arch: self.arch,
            daemon_version: self.daemon_version,
            protocol_version: u16::try_from(self.protocol_version).unwrap_or_default(),
            status: parse_machine_status(&self.status),
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
            connected_at: parse_opt_ts(self.connected_at),
            last_seen_at: parse_opt_ts(self.last_seen_at),
            workspace_count: u32::try_from(workspaces.len()).unwrap_or(u32::MAX),
            providers,
            workspaces,
            public_key: self.public_key,
        }
    }
}

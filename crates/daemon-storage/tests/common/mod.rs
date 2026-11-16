#![allow(dead_code)]
//! Shared fixtures for storage integration tests.

//! End-to-end tests for the SQLite storage crate.

use chrono::{Duration, TimeZone, Utc};
use fleet_core_common::Timestamp;
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentPermissionRequest, AgentSession, PermissionStatus, ProviderInfo,
    ProviderKind, SessionStatus,
};
use fleet_core_protocol::git::GitFileStatus;
use fleet_core_protocol::machine::{Machine, MachineStatus};
use fleet_core_protocol::workspace::{EntryType, Workspace, WorkspaceEntry, WorkspaceSummary};
use serde_json::{json, Value};

pub fn ts(seconds: i64) -> Timestamp {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .expect("valid fixed timestamp")
}

pub fn machine(id: &str, name: &str) -> Machine {
    Machine {
        id: id.to_owned(),
        name: name.to_owned(),
        hostname: format!("{name}.local"),
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        daemon_version: "0.1.0".to_owned(),
        protocol_version: 1,
        status: MachineStatus::Online,
        created_at: ts(1_700_000_000),
        updated_at: ts(1_700_000_100),
        connected_at: Some(ts(1_700_000_050)),
        last_seen_at: Some(ts(1_700_000_090)),
        providers: vec![ProviderInfo {
            id: "mock".to_owned(),
            kind: ProviderKind::Mock,
            display_name: "Mock Agent".to_owned(),
            version: Some("0.1.0".to_owned()),
            available: true,
            capabilities: AgentCapabilities::full(),
            models: vec!["mock-fast".to_owned(), "mock-smart".to_owned()],
        }],
        workspaces: vec![WorkspaceSummary {
            id: "ws_1".to_owned(),
            name: "alpha".to_owned(),
            root_path: "/tmp/alpha".to_owned(),
            is_git_repository: true,
        }],
        workspace_count: 1,
        public_key: Some("public-key".to_owned()),
    }
}

pub fn workspace(id: &str, machine_id: &str, name: &str) -> Workspace {
    Workspace {
        id: id.to_owned(),
        machine_id: machine_id.to_owned(),
        project_id: Some("project_1".to_owned()),
        name: name.to_owned(),
        root_path: format!("/tmp/{name}"),
        is_git_repository: true,
        created_at: ts(1_700_000_000),
        updated_at: ts(1_700_000_100),
    }
}

pub fn session(
    id: &str,
    machine_id: &str,
    created_at: Timestamp,
    status: SessionStatus,
    metadata: Value,
) -> AgentSession {
    AgentSession {
        id: id.to_owned(),
        machine_id: machine_id.to_owned(),
        workspace_id: None,
        provider: "mock".to_owned(),
        agent_type: "mock-agent".to_owned(),
        status,
        cwd: "/tmp/alpha".to_owned(),
        title: Some(format!("session {id}")),
        created_at,
        updated_at: created_at + Duration::seconds(5),
        last_error: None,
        metadata,
    }
}

pub fn permission(
    id: &str,
    session_id: &str,
    created_at: Timestamp,
    status: PermissionStatus,
) -> AgentPermissionRequest {
    AgentPermissionRequest {
        id: id.to_owned(),
        session_id: session_id.to_owned(),
        turn_id: None,
        tool_call_id: None,
        tool_name: "Bash".to_owned(),
        description: "Run a command".to_owned(),
        input: json!({"command": "ls"}),
        options: Vec::new(),
        status,
        created_at,
        resolved_at: None,
        expires_at: Some(created_at + Duration::minutes(5)),
    }
}

pub fn entry(path: &str, name: &str, entry_type: EntryType) -> WorkspaceEntry {
    WorkspaceEntry {
        path: path.to_owned(),
        name: name.to_owned(),
        entry_type,
        size: Some(42),
        modified_at: Some(ts(1_700_000_123)),
        git_status: Some(GitFileStatus::Modified),
        ignored: true,
    }
}

pub fn paths(entries: &[WorkspaceEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.path.clone()).collect()
}

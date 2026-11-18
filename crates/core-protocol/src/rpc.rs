//! RPC method names and request/response payloads for the machine connection.
//!
//! The agent admin is a thin, authorization-aware proxy: every method below is
//! forwarded to the daemon that owns the machine. Method names are stable API.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent::{
    AgentMessage, AgentPermissionRequest, AgentQuestionRequest, AgentSession, AgentToolCall,
    AgentTurn, ProviderInfo,
};
use crate::common::{SessionId, WorkspaceId};
use crate::git::{
    GitBranch, GitCheckoutOptions, GitCheckoutResult, GitCommitOptions, GitCommitResult, GitDiff,
    GitDiffOptions, GitDiffSummary, GitLogEntry, GitLogRequest, GitPathsRequest, GitStatus,
};
use crate::workspace::{
    DirectoryListing, FileContent, ListDirectoryRequest, NotModified, RegisterWorkspaceRequest,
    SearchRequest, SearchResponse, Workspace,
};

/// Method name constants, shared by daemon and agent admin.
pub mod methods {
    // connection lifecycle
    pub const HELLO: &str = "connection.hello";
    pub const AUTH_CHALLENGE: &str = "connection.auth_challenge";
    pub const AUTH_RESPONSE: &str = "connection.auth";
    pub const ACCEPTED: &str = "connection.accepted";
    pub const HEARTBEAT: &str = "connection.heartbeat";
    pub const HEARTBEAT_ACK: &str = "connection.heartbeat_ack";
    pub const DISCONNECT: &str = "connection.disconnect";

    // durable event delivery
    pub const EVENTS_ACK: &str = "events.ack";

    // health
    pub const HEALTH: &str = "health.get";

    // providers / agents / drivers / sessions
    pub const PROVIDERS_LIST: &str = "agent.providers.list";
    pub const AGENTS_LIST: &str = "agent.agents.list";
    pub const AGENTS_PROBE: &str = "agent.agents.probe";
    pub const DRIVERS_LIST: &str = "agent.drivers.list";
    pub const SESSIONS_CREATE: &str = "agent.sessions.create";
    pub const SESSIONS_LIST: &str = "agent.sessions.list";
    pub const SESSIONS_GET: &str = "agent.sessions.get";
    pub const SESSIONS_SEND_MESSAGE: &str = "agent.sessions.send_message";
    pub const SESSIONS_INTERRUPT: &str = "agent.sessions.interrupt";
    pub const SESSIONS_RESUME: &str = "agent.sessions.resume";
    pub const SESSIONS_SHUTDOWN: &str = "agent.sessions.shutdown";
    pub const SESSIONS_DELETE: &str = "agent.sessions.delete";
    pub const PERMISSIONS_LIST: &str = "agent.permissions.list";
    pub const PERMISSIONS_RESPOND: &str = "agent.permissions.respond";
    pub const QUESTIONS_LIST: &str = "agent.questions.list";
    pub const QUESTIONS_RESPOND: &str = "agent.questions.respond";

    // workspaces
    pub const WORKSPACES_LIST: &str = "workspace.list";
    pub const WORKSPACES_REGISTER: &str = "workspace.register";
    pub const WORKSPACES_GET: &str = "workspace.get";
    pub const WORKSPACES_REMOVE: &str = "workspace.remove";
    pub const WORKSPACES_TREE: &str = "workspace.tree.list";
    pub const WORKSPACES_FILE_READ: &str = "workspace.file.read";
    pub const WORKSPACES_FILE_SEARCH: &str = "workspace.file.search";

    // git
    pub const GIT_STATUS: &str = "git.status";
    pub const GIT_DIFF: &str = "git.diff";
    pub const GIT_DIFF_SUMMARY: &str = "git.diff.summary";
    pub const GIT_BRANCHES: &str = "git.branches";
    pub const GIT_LOG: &str = "git.log";

    // git writes (authorized and audited by the agent admin)
    pub const GIT_STAGE: &str = "git.stage";
    pub const GIT_UNSTAGE: &str = "git.unstage";
    pub const GIT_COMMIT: &str = "git.commit";
    pub const GIT_CHECKOUT: &str = "git.checkout";
}

/// Request body for creating an agent session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreateAgentSessionRequest {
    /// Agent definition id. Preferred over the legacy `provider` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_definition_id: Option<String>,
    /// Legacy provider id. Still accepted and resolved to a definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    /// Optional explicitly requested working directory. When omitted the daemon
    /// uses the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

/// Request body for probing one agent definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Agent definition id.
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ListSessionsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRequest {
    pub session_id: SessionId,
}

/// Request body for `events.ack`: the highest durable outbox cursor applied by
/// the agent admin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckEventsRequest {
    pub cursor: u64,
}

/// Payload for `agent.sessions.send_message`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendMessageRpcRequest {
    pub session_id: SessionId,
    #[serde(flatten)]
    pub message: crate::agent::SendMessageRequest,
}

/// Control-plane request body for `POST /api/v1/sessions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateSessionRpcRequest {
    pub machine_id: crate::common::MachineId,
    #[serde(flatten)]
    pub create: CreateAgentSessionRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session: AgentSession,
    #[serde(default)]
    pub turns: Vec<AgentTurn>,
    #[serde(default)]
    pub messages: Vec<AgentMessage>,
    #[serde(default)]
    pub tool_calls: Vec<AgentToolCall>,
    #[serde(default)]
    pub permissions: Vec<AgentPermissionRequest>,
    #[serde(default)]
    pub questions: Vec<AgentQuestionRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListPermissionsRequest {
    pub session_id: SessionId,
}

/// Workspace-scoped request wrapper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRequest {
    pub workspace_id: WorkspaceId,
}

/// Control-plane request body for `POST /api/v1/workspaces`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterWorkspaceRpcRequest {
    pub machine_id: crate::common::MachineId,
    #[serde(flatten)]
    pub register: RegisterWorkspaceRequest,
}

/// Control-plane request body for permission approve/reject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RespondPermissionHttpRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeListRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub listing: ListDirectoryRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileReadRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub read: crate::workspace::ReadFileRequest,
}

/// Either fresh content or a `304`-style notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileReadResponse {
    Content(FileContent),
    NotModified(NotModified),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileSearchRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub search: SearchRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitDiffRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub options: GitDiffOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitLogRpcRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub log: GitLogRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitPathsRpcRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub paths: GitPathsRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitCommitRpcRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub commit: GitCommitOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitCheckoutRpcRequest {
    pub workspace_id: WorkspaceId,
    #[serde(flatten)]
    pub checkout: GitCheckoutOptions,
}

/// All workspaces live on one machine; this is what the daemon advertises.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DaemonSnapshot {
    pub providers: Vec<ProviderInfo>,
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub sessions: Vec<AgentSession>,
}

/// Type aliases documenting the expected response shapes.
pub type ProvidersResponse = Vec<ProviderInfo>;
pub type WorkspacesResponse = Vec<Workspace>;
pub type SessionsResponse = Vec<AgentSession>;
pub type PermissionsResponse = Vec<AgentPermissionRequest>;
pub type TreeResponse = DirectoryListing;
pub type GitStatusResponse = GitStatus;
pub type GitDiffResponse = GitDiff;
pub type GitDiffSummaryResponse = GitDiffSummary;
pub type GitBranchesResponse = Vec<GitBranch>;
pub type GitLogResponse = Vec<GitLogEntry>;
pub type GitCommitResponse = GitCommitResult;
pub type GitCheckoutResponse = GitCheckoutResult;
pub type SearchResponsePayload = SearchResponse;
pub type RegisterWorkspaceResponse = Workspace;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_request_omits_empty_fields() {
        let req = CreateAgentSessionRequest::default();
        assert_eq!(serde_json::to_string(&req).unwrap(), "{}");
    }

    #[test]
    fn tree_request_flattens_listing() {
        let req = TreeListRequest {
            workspace_id: "ws_1".into(),
            listing: ListDirectoryRequest {
                path: "src".into(),
                cursor: None,
                limit: Some(10),
                show_ignored: None,
            },
        };
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["workspace_id"], "ws_1");
        assert_eq!(value["path"], "src");
        assert_eq!(value["limit"], 10);
    }

    #[test]
    fn file_read_response_is_untagged() {
        let content = FileReadResponse::Content(FileContent {
            path: "a.rs".into(),
            content: "fn main() {}".into(),
            encoding: "utf-8".into(),
            size: 13,
            etag: "abc".into(),
            truncated: false,
            is_binary: false,
            language: Some("rust".into()),
            modified_at: None,
        });
        let json = serde_json::to_string(&content).unwrap();
        let back: FileReadResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(content, back);
    }

    #[test]
    fn method_names_are_stable() {
        assert_eq!(methods::SESSIONS_CREATE, "agent.sessions.create");
        assert_eq!(methods::SESSIONS_DELETE, "agent.sessions.delete");
        assert_eq!(methods::WORKSPACES_TREE, "workspace.tree.list");
        assert_eq!(methods::GIT_DIFF_SUMMARY, "git.diff.summary");
    }
}

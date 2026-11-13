use std::path::Path;
use std::sync::Arc;

use agent_fleet_common::{error_codes, now, Ack, ApiError};
use agent_fleet_connection_gateway::DaemonRequestHandler;
use agent_fleet_protocol::agent::{
    AgentPermissionRequest, AgentSession, AgentStartConfig, PermissionDecision,
    RespondPermissionRequest,
};
use agent_fleet_protocol::machine::HealthPayload;
use agent_fleet_protocol::rpc::{
    methods, CreateAgentSessionRequest, FileReadRequest, FileReadResponse, FileSearchRequest,
    GitDiffRequest, GitLogRpcRequest, ListPermissionsRequest, ListSessionsRequest,
    SendMessageRpcRequest, SessionDetail, SessionRequest, TreeListRequest, WorkspaceRequest,
};
use agent_fleet_protocol::workspace::{NotModified, RegisterWorkspaceRequest};
use async_trait::async_trait;
use serde_json::Value;

use crate::events::WatcherManager;
use crate::services::Services;

/// Dispatches control-plane RPCs to the local services.
pub struct DaemonHandler {
    services: Arc<Services>,
    watchers: WatcherManager,
}

impl DaemonHandler {
    pub fn new(services: Arc<Services>, watchers: WatcherManager) -> Self {
        Self { services, watchers }
    }
}

#[async_trait]
impl DaemonRequestHandler for DaemonHandler {
    async fn handle(&self, method: &str, payload: Value) -> Result<Value, ApiError> {
        match method {
            methods::HEALTH => {
                let sessions = self.services.runtime.list_sessions().await;
                let active = sessions.iter().filter(|s| s.status.is_active()).count();
                to_value(HealthPayload {
                    machine_id: self.services.machine_id.clone(),
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    protocol_version: agent_fleet_protocol::PROTOCOL_VERSION,
                    uptime_secs: self.services.started_at.elapsed().as_secs(),
                    active_sessions: active as u32,
                    timestamp: now(),
                })
            }

            methods::PROVIDERS_LIST => to_value(self.services.runtime.providers()),

            methods::SESSIONS_CREATE => {
                let req: CreateAgentSessionRequest = from_value(payload)?;
                let (workspace_id, cwd) = self.resolve_cwd(&req).await?;
                let provider = req
                    .provider
                    .clone()
                    .unwrap_or_else(|| self.services.default_provider.clone());
                let start = AgentStartConfig {
                    workspace_id,
                    cwd,
                    provider: Some(provider.clone()),
                    model: req.model,
                    title: req.title,
                    system_prompt: req.system_prompt,
                    allowed_tools: req.allowed_tools,
                    env: req.env,
                    extra: req.extra,
                };
                let session = self
                    .services
                    .runtime
                    .start_session(&self.services.machine_id, &provider, start)
                    .await
                    .map_err(runtime_error)?;
                to_value(session)
            }

            methods::SESSIONS_LIST => {
                let _req: ListSessionsRequest = from_value(payload).unwrap_or_default();
                let sessions: Vec<AgentSession> = self.services.runtime.list_sessions().await;
                to_value(sessions)
            }

            methods::SESSIONS_GET => {
                let req: SessionRequest = from_value(payload)?;
                match self.services.runtime.session_detail(&req.session_id).await {
                    Some(detail) => to_value::<SessionDetail>(detail),
                    None => Err(ApiError::not_found(format!(
                        "session {} not found",
                        req.session_id
                    ))),
                }
            }

            methods::SESSIONS_SEND_MESSAGE => {
                let req: SendMessageRpcRequest = from_value(payload)?;
                self.services
                    .runtime
                    .send_message(&req.session_id, req.message)
                    .await
                    .map_err(runtime_error)?;
                to_value(Ack::ok())
            }

            methods::SESSIONS_INTERRUPT => {
                let req: SessionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .interrupt(&req.session_id)
                    .await
                    .map_err(runtime_error)?;
                to_value(Ack::ok())
            }

            methods::SESSIONS_RESUME => {
                let req: SessionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .resume(&req.session_id)
                    .await
                    .map_err(runtime_error)?;
                to_value(Ack::ok())
            }

            methods::SESSIONS_SHUTDOWN => {
                let req: SessionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .shutdown_session(&req.session_id)
                    .await
                    .map_err(runtime_error)?;
                to_value(Ack::ok())
            }

            methods::PERMISSIONS_LIST => {
                let req: ListPermissionsRequest = from_value(payload)?;
                let detail = self
                    .services
                    .runtime
                    .session_detail(&req.session_id)
                    .await
                    .ok_or_else(|| {
                        ApiError::not_found(format!("session {} not found", req.session_id))
                    })?;
                to_value::<Vec<AgentPermissionRequest>>(detail.permissions)
            }

            methods::PERMISSIONS_RESPOND => {
                let req: RespondPermissionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .respond_permission(PermissionDecision {
                        permission_id: req.permission_id,
                        approve: req.approve,
                        message: req.message,
                    })
                    .await
                    .map_err(runtime_error)?;
                to_value(Ack::ok())
            }

            methods::WORKSPACES_LIST => {
                let workspaces = self
                    .services
                    .workspaces
                    .list(None)
                    .await
                    .map_err(ApiError::from)?;
                to_value(workspaces)
            }

            methods::WORKSPACES_REGISTER => {
                let req: RegisterWorkspaceRequest = from_value(payload)?;
                let workspace = self
                    .services
                    .workspaces
                    .register(&self.services.machine_id, req)
                    .await
                    .map_err(ApiError::from)?;
                self.watchers.ensure(workspace.clone()).await;
                to_value(workspace)
            }

            methods::WORKSPACES_GET => {
                let req: WorkspaceRequest = from_value(payload)?;
                let workspace = self
                    .services
                    .workspaces
                    .get(&req.workspace_id)
                    .await
                    .map_err(ApiError::from)?;
                to_value(workspace)
            }

            methods::WORKSPACES_REMOVE => {
                let req: WorkspaceRequest = from_value(payload)?;
                self.services
                    .workspaces
                    .remove(&req.workspace_id)
                    .await
                    .map_err(ApiError::from)?;
                to_value(Ack::ok())
            }

            methods::WORKSPACES_TREE => {
                let req: TreeListRequest = from_value(payload)?;
                let listing = self
                    .services
                    .workspaces
                    .list_directory(&req.workspace_id, req.listing)
                    .await
                    .map_err(ApiError::from)?;
                to_value(listing)
            }

            methods::WORKSPACES_FILE_READ => {
                let req: FileReadRequest = from_value(payload)?;
                let path = req.read.path.clone();
                let etag = req.read.etag.clone();
                let content = self
                    .services
                    .workspaces
                    .read_file(&req.workspace_id, req.read)
                    .await
                    .map_err(ApiError::from)?;
                match content {
                    Some(content) => to_value(FileReadResponse::Content(content)),
                    None => to_value(FileReadResponse::NotModified(NotModified {
                        path,
                        etag: etag.unwrap_or_default(),
                    })),
                }
            }

            methods::WORKSPACES_FILE_SEARCH => {
                let req: FileSearchRequest = from_value(payload)?;
                let response = self
                    .services
                    .workspaces
                    .search(&req.workspace_id, req.search)
                    .await
                    .map_err(ApiError::from)?;
                to_value(response)
            }

            methods::GIT_STATUS => {
                let req: WorkspaceRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                let status = self
                    .services
                    .git
                    .status(Path::new(&root))
                    .await
                    .map_err(ApiError::from)?;
                to_value(status)
            }

            methods::GIT_DIFF => {
                let req: GitDiffRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                let diff = self
                    .services
                    .git
                    .diff(Path::new(&root), &req.options)
                    .await
                    .map_err(ApiError::from)?;
                to_value(diff)
            }

            methods::GIT_DIFF_SUMMARY => {
                let req: GitDiffRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                let summary = self
                    .services
                    .git
                    .diff_summary(Path::new(&root), &req.options)
                    .await
                    .map_err(ApiError::from)?;
                to_value(summary)
            }

            methods::GIT_BRANCHES => {
                let req: WorkspaceRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                let branches = self
                    .services
                    .git
                    .branches(Path::new(&root))
                    .await
                    .map_err(ApiError::from)?;
                to_value(branches)
            }

            methods::GIT_LOG => {
                let req: GitLogRpcRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                let entries = self
                    .services
                    .git
                    .log(Path::new(&root), &req.log)
                    .await
                    .map_err(ApiError::from)?;
                to_value(entries)
            }

            other => Err(ApiError::new(
                error_codes::UNSUPPORTED,
                format!("method `{other}` is not supported by this daemon"),
            )),
        }
    }

    async fn on_connected(&self, accepted: &agent_fleet_protocol::connection::ConnectionAccepted) {
        tracing::info!(
            machine = %accepted.machine_id,
            connection = %accepted.connection_id,
            "registered with control plane"
        );
    }

    async fn on_disconnected(&self, reason: &str) {
        tracing::warn!(reason, "control plane connection lost");
    }
}

impl DaemonHandler {
    async fn workspace_root(&self, workspace_id: &str) -> Result<String, ApiError> {
        let workspace = self
            .services
            .workspaces
            .get(workspace_id)
            .await
            .map_err(ApiError::from)?;
        Ok(workspace.root_path)
    }

    async fn resolve_cwd(
        &self,
        req: &CreateAgentSessionRequest,
    ) -> Result<(Option<String>, String), ApiError> {
        match (&req.workspace_id, &req.cwd) {
            (Some(workspace_id), requested) => {
                let workspace = self
                    .services
                    .workspaces
                    .get(workspace_id)
                    .await
                    .map_err(ApiError::from)?;
                let cwd = match requested {
                    None => workspace.root_path.clone(),
                    Some(cwd) => {
                        let root = tokio::fs::canonicalize(&workspace.root_path)
                            .await
                            .map_err(|err| {
                                ApiError::invalid(format!("workspace root unavailable: {err}"))
                            })?;
                        let candidate = tokio::fs::canonicalize(cwd).await.map_err(|err| {
                            ApiError::invalid(format!("cwd is not accessible: {err}"))
                        })?;
                        if !candidate.starts_with(&root) {
                            return Err(ApiError::new(
                                error_codes::PATH_DENIED,
                                "cwd must be inside the workspace root",
                            ));
                        }
                        candidate.to_string_lossy().to_string()
                    }
                };
                Ok((Some(workspace_id.clone()), cwd))
            }
            (None, Some(cwd)) => {
                let candidate = tokio::fs::canonicalize(cwd)
                    .await
                    .map_err(|err| ApiError::invalid(format!("cwd is not accessible: {err}")))?;
                if !candidate.is_dir() {
                    return Err(ApiError::invalid("cwd must be a directory"));
                }
                Ok((None, candidate.to_string_lossy().to_string()))
            }
            (None, None) => Err(ApiError::invalid(
                "either workspace_id or cwd is required to create a session",
            )),
        }
    }
}

fn from_value<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value)
        .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|err| ApiError::internal(err.to_string()))
}

fn runtime_error(err: impl std::fmt::Display) -> ApiError {
    let message = err.to_string();
    if message.contains("not found") {
        ApiError::not_found(message)
    } else {
        ApiError::internal(message)
    }
}

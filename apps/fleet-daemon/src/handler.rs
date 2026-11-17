use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use fleet_core_common::{error_codes, now, Ack, ApiError};
use fleet_core_connection::DaemonRequestHandler;
use fleet_core_protocol::agent::{
    AgentPermissionRequest, AgentQuestionRequest, AgentSession, AgentStartConfig,
    ListQuestionsRequest, PermissionDecision, QuestionDecision, RespondPermissionRequest,
    RespondQuestionRequest,
};
use fleet_core_protocol::machine::HealthPayload;
use fleet_core_protocol::rpc::{
    methods, AckEventsRequest, CreateAgentSessionRequest, FileReadRequest, FileReadResponse,
    FileSearchRequest, GitCheckoutRpcRequest, GitCommitRpcRequest, GitDiffRequest,
    GitLogRpcRequest, GitPathsRpcRequest, ListPermissionsRequest, ListSessionsRequest,
    SendMessageRpcRequest, SessionDetail, SessionRequest, TreeListRequest, WorkspaceRequest,
};
use fleet_core_protocol::workspace::{NotModified, RegisterWorkspaceRequest};
use serde_json::{json, Value};

use crate::events::WatcherManager;
use crate::services::Services;

/// Dispatches fleet-admin RPCs to the local services.
pub struct DaemonHandler {
    services: Arc<Services>,
    watchers: WatcherManager,
}

impl DaemonHandler {
    pub fn new(services: Arc<Services>, watchers: WatcherManager) -> Self {
        Self { services, watchers }
    }

    fn ensure_git_writes_allowed(&self) -> Result<(), ApiError> {
        if self.services.config.git.allow_writes {
            Ok(())
        } else {
            Err(ApiError::new(
                error_codes::FORBIDDEN,
                "Git write operations are disabled on this daemon",
            ))
        }
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
                    machine_name: Some(
                        self.services
                            .config
                            .machine
                            .name
                            .clone()
                            .unwrap_or_else(fleet_core_connection::default_machine_name),
                    ),
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
                    uptime_secs: self.services.started_at.elapsed().as_secs(),
                    active_sessions: active as u32,
                    timestamp: now(),
                })
            }

            methods::EVENTS_ACK => {
                let req: AckEventsRequest = from_value(payload)?;
                let cursor = self
                    .services
                    .storage
                    .outbox
                    .ack(req.cursor)
                    .await
                    .map_err(ApiError::from)?;
                to_value(json!({ "cursor": cursor }))
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
                    .map_err(ApiError::from)?;
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
                    .map_err(ApiError::from)?;
                to_value(Ack::ok())
            }

            methods::SESSIONS_INTERRUPT => {
                let req: SessionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .interrupt(&req.session_id)
                    .await
                    .map_err(ApiError::from)?;
                to_value(Ack::ok())
            }

            methods::SESSIONS_RESUME => {
                let req: SessionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .resume(&req.session_id)
                    .await
                    .map_err(ApiError::from)?;
                to_value(Ack::ok())
            }

            methods::SESSIONS_SHUTDOWN => {
                let req: SessionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .shutdown_session(&req.session_id)
                    .await
                    .map_err(ApiError::from)?;
                to_value(Ack::ok())
            }

            methods::SESSIONS_DELETE => {
                let req: SessionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .delete_session(&req.session_id)
                    .await
                    .map_err(ApiError::from)?;
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
                        session_id: req.session_id,
                        approve: req.approve,
                        option_id: req.option_id,
                        message: req.message,
                    })
                    .await
                    .map_err(ApiError::from)?;
                to_value(Ack::ok())
            }

            methods::QUESTIONS_LIST => {
                let req: ListQuestionsRequest = from_value(payload)?;
                let detail = self
                    .services
                    .runtime
                    .session_detail(&req.session_id)
                    .await
                    .ok_or_else(|| {
                        ApiError::not_found(format!("session {} not found", req.session_id))
                    })?;
                to_value::<Vec<AgentQuestionRequest>>(detail.questions)
            }

            methods::QUESTIONS_RESPOND => {
                let req: RespondQuestionRequest = from_value(payload)?;
                self.services
                    .runtime
                    .respond_question(QuestionDecision {
                        question_id: req.question_id,
                        session_id: req.session_id,
                        answers: req.answers.unwrap_or_default(),
                        reject: req.reject,
                        message: req.message,
                    })
                    .await
                    .map_err(ApiError::from)?;
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
                let workspace = self
                    .services
                    .workspaces
                    .get(&req.workspace_id)
                    .await
                    .map_err(ApiError::from)?;
                let root = Path::new(&workspace.root_path);
                let active =
                    self.services
                        .runtime
                        .list_sessions()
                        .await
                        .into_iter()
                        .any(|session| {
                            session.status.is_active()
                                && (session.workspace_id.as_deref() == Some(&req.workspace_id)
                                    || Path::new(&session.cwd).starts_with(root))
                        });
                if active {
                    return Err(ApiError::new(
                        error_codes::CONFLICT,
                        "workspace has active sessions; stop them before deleting the workspace",
                    ));
                }
                self.services
                    .workspaces
                    .remove(&req.workspace_id)
                    .await
                    .map_err(ApiError::from)?;
                self.watchers.remove(&req.workspace_id).await;
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

            methods::GIT_STAGE | methods::GIT_UNSTAGE => {
                self.ensure_git_writes_allowed()?;
                let req: GitPathsRpcRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                if method == methods::GIT_STAGE {
                    self.services
                        .git
                        .stage(Path::new(&root), &req.paths.paths)
                        .await
                        .map_err(ApiError::from)?;
                } else {
                    self.services
                        .git
                        .unstage(Path::new(&root), &req.paths.paths)
                        .await
                        .map_err(ApiError::from)?;
                }
                to_value(Ack::ok())
            }

            methods::GIT_COMMIT => {
                self.ensure_git_writes_allowed()?;
                let req: GitCommitRpcRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                let result = self
                    .services
                    .git
                    .commit(Path::new(&root), &req.commit)
                    .await
                    .map_err(ApiError::from)?;
                to_value(result)
            }

            methods::GIT_CHECKOUT => {
                self.ensure_git_writes_allowed()?;
                let req: GitCheckoutRpcRequest = from_value(payload)?;
                let root = self.workspace_root(&req.workspace_id).await?;
                let result = self
                    .services
                    .git
                    .checkout(Path::new(&root), &req.checkout)
                    .await
                    .map_err(ApiError::from)?;
                to_value(result)
            }

            other => Err(ApiError::new(
                error_codes::UNSUPPORTED,
                format!("method `{other}` is not supported by this daemon"),
            )),
        }
    }

    async fn on_connected(&self, accepted: &fleet_core_protocol::connection::ConnectionAccepted) {
        tracing::info!(
            machine = %accepted.machine_id,
            connection = %accepted.connection_id,
            "registered with agent admin"
        );
        // A rotation proof is single-use; discard the previous key once the
        // agent admin has accepted the new one.
        if let Err(err) =
            fleet_core_auth::MachineIdentity::clear_previous(&self.services.config.machine.key_file)
        {
            tracing::warn!(error = %err, "failed to remove the previous machine key");
        }
    }

    async fn on_disconnected(&self, reason: &str) {
        tracing::warn!(reason, "agent admin connection lost");
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
                // A bare `cwd` must still live inside a workspace registered
                // with this daemon; otherwise any session could be pointed at
                // an arbitrary directory on the host.
                let workspaces = self
                    .services
                    .workspaces
                    .list(Some(&self.services.machine_id))
                    .await
                    .map_err(ApiError::from)?;
                let mut roots = Vec::new();
                for workspace in &workspaces {
                    if let Ok(root) = tokio::fs::canonicalize(&workspace.root_path).await {
                        roots.push(root);
                    }
                }
                if !is_within_any_root(&candidate, &roots) {
                    return Err(ApiError::new(
                        error_codes::PATH_DENIED,
                        "cwd must be inside a registered workspace; register the workspace or pass workspace_id",
                    ));
                }
                Ok((None, candidate.to_string_lossy().to_string()))
            }
            (None, None) => Err(ApiError::invalid(
                "either workspace_id or cwd is required to create a session",
            )),
        }
    }
}

/// Whether a canonical path lives inside any of the allowed workspace roots.
fn is_within_any_root(candidate: &std::path::Path, roots: &[std::path::PathBuf]) -> bool {
    roots.iter().any(|root| candidate.starts_with(root))
}

fn from_value<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value)
        .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|err| ApiError::internal(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::is_within_any_root;
    use std::path::{Path, PathBuf};

    #[test]
    fn cwd_must_stay_within_a_registered_workspace_root() {
        let roots = vec![PathBuf::from("/srv/work/a"), PathBuf::from("/srv/work/b")];
        assert!(is_within_any_root(Path::new("/srv/work/a"), &roots));
        assert!(is_within_any_root(Path::new("/srv/work/a/src"), &roots));
        assert!(is_within_any_root(Path::new("/srv/work/b/pkg"), &roots));
        assert!(!is_within_any_root(Path::new("/srv/work"), &roots));
        assert!(!is_within_any_root(Path::new("/srv/work/a2"), &roots));
        assert!(!is_within_any_root(Path::new("/etc"), &roots));
        assert!(!is_within_any_root(Path::new("/srv/work/a"), &[]));
    }
}

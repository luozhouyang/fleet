//! Git read operations proxied to the machine that owns a workspace.

use agent_fleet_protocol::git::{GitBranch, GitDiff, GitDiffSummary, GitLogEntry, GitStatus};
use agent_fleet_protocol::rpc::{methods, GitDiffRequest, GitLogRpcRequest, WorkspaceRequest};

use crate::error::AppError;
use crate::services::workspace_service::WorkspaceService;
use crate::services::{forward, parse_response, to_payload};
use crate::state::AppState;

/// Git operations exposed by the control plane.
pub struct GitService;

impl GitService {
    /// Proxy `git.status`.
    pub async fn status(state: &AppState, workspace_id: &str) -> Result<GitStatus, AppError> {
        let value = Self::workspace_call(state, workspace_id, methods::GIT_STATUS).await?;
        parse_response(value, "git.status")
    }

    /// Proxy `git.branches`.
    pub async fn branches(
        state: &AppState,
        workspace_id: &str,
    ) -> Result<Vec<GitBranch>, AppError> {
        let value = Self::workspace_call(state, workspace_id, methods::GIT_BRANCHES).await?;
        parse_response(value, "git.branches")
    }

    /// Proxy `git.log`.
    pub async fn log(
        state: &AppState,
        workspace_id: &str,
        request: GitLogRpcRequest,
    ) -> Result<Vec<GitLogEntry>, AppError> {
        let workspace = WorkspaceService::get(state, workspace_id).await?;
        let payload = to_payload(&GitLogRpcRequest {
            workspace_id: workspace.id,
            log: request.log,
        })?;
        let value = forward(state, &workspace.machine_id, methods::GIT_LOG, payload).await?;
        parse_response(value, "git.log")
    }

    /// Proxy `git.diff`.
    pub async fn diff(
        state: &AppState,
        workspace_id: &str,
        request: GitDiffRequest,
    ) -> Result<GitDiff, AppError> {
        let workspace = WorkspaceService::get(state, workspace_id).await?;
        let payload = to_payload(&GitDiffRequest {
            workspace_id: workspace.id,
            options: request.options,
        })?;
        let value = forward(state, &workspace.machine_id, methods::GIT_DIFF, payload).await?;
        parse_response(value, "git.diff")
    }

    /// Proxy `git.diff.summary`.
    pub async fn diff_summary(
        state: &AppState,
        workspace_id: &str,
        request: GitDiffRequest,
    ) -> Result<GitDiffSummary, AppError> {
        let workspace = WorkspaceService::get(state, workspace_id).await?;
        let payload = to_payload(&GitDiffRequest {
            workspace_id: workspace.id,
            options: request.options,
        })?;
        let value = forward(
            state,
            &workspace.machine_id,
            methods::GIT_DIFF_SUMMARY,
            payload,
        )
        .await?;
        parse_response(value, "git.diff.summary")
    }

    async fn workspace_call(
        state: &AppState,
        workspace_id: &str,
        method: &str,
    ) -> Result<serde_json::Value, AppError> {
        let workspace = WorkspaceService::get(state, workspace_id).await?;
        let payload = to_payload(&WorkspaceRequest {
            workspace_id: workspace.id,
        })?;
        forward(state, &workspace.machine_id, method, payload).await
    }
}

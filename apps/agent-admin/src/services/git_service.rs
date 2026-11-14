//! Git read operations proxied to the machine that owns a workspace.

use agent_fleet_protocol::git::{
    GitBranch, GitCheckoutOptions, GitCheckoutResult, GitCommitOptions, GitCommitResult, GitDiff,
    GitDiffSummary, GitLogEntry, GitPathsRequest, GitStatus,
};
use agent_fleet_protocol::rpc::{
    methods, GitCheckoutRpcRequest, GitCommitRpcRequest, GitDiffRequest, GitLogRpcRequest,
    GitPathsRpcRequest, WorkspaceRequest,
};

use crate::error::AppError;
use crate::services::workspace_service::WorkspaceService;
use crate::services::{forward, parse_response, to_payload};
use crate::state::AppState;

/// Git operations exposed by the agent admin.
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

    /// Proxy `git.stage`.
    pub async fn stage(
        state: &AppState,
        workspace_id: &str,
        paths: Vec<String>,
    ) -> Result<(), AppError> {
        Self::write_paths(state, workspace_id, paths, methods::GIT_STAGE).await
    }

    /// Proxy `git.unstage`.
    pub async fn unstage(
        state: &AppState,
        workspace_id: &str,
        paths: Vec<String>,
    ) -> Result<(), AppError> {
        Self::write_paths(state, workspace_id, paths, methods::GIT_UNSTAGE).await
    }

    /// Proxy `git.commit`.
    pub async fn commit(
        state: &AppState,
        workspace_id: &str,
        commit: GitCommitOptions,
    ) -> Result<GitCommitResult, AppError> {
        let workspace = WorkspaceService::get(state, workspace_id).await?;
        let payload = to_payload(&GitCommitRpcRequest {
            workspace_id: workspace.id,
            commit,
        })?;
        let value = forward(state, &workspace.machine_id, methods::GIT_COMMIT, payload).await?;
        parse_response(value, "git.commit")
    }

    /// Proxy `git.checkout`.
    pub async fn checkout(
        state: &AppState,
        workspace_id: &str,
        checkout: GitCheckoutOptions,
    ) -> Result<GitCheckoutResult, AppError> {
        let workspace = WorkspaceService::get(state, workspace_id).await?;
        let payload = to_payload(&GitCheckoutRpcRequest {
            workspace_id: workspace.id,
            checkout,
        })?;
        let value = forward(state, &workspace.machine_id, methods::GIT_CHECKOUT, payload).await?;
        parse_response(value, "git.checkout")
    }

    async fn write_paths(
        state: &AppState,
        workspace_id: &str,
        paths: Vec<String>,
        method: &str,
    ) -> Result<(), AppError> {
        let workspace = WorkspaceService::get(state, workspace_id).await?;
        let payload = to_payload(&GitPathsRpcRequest {
            workspace_id: workspace.id,
            paths: GitPathsRequest { paths },
        })?;
        forward(state, &workspace.machine_id, method, payload).await?;
        Ok(())
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

//! Git read endpoints, proxied to the owning machine.

use agent_fleet_protocol::git::{
    GitBranch, GitDiff, GitDiffOptions, GitDiffSummary, GitLogEntry, GitLogRequest, GitStatus,
};
use agent_fleet_protocol::rpc::{GitDiffRequest, GitLogRpcRequest};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::error::AppError;
use crate::services::git_service::GitService;
use crate::state::AppState;

/// Query for the two diff endpoints.
#[derive(Debug, Deserialize, Default)]
pub struct DiffQuery {
    /// Workspace-relative path filter.
    #[serde(default)]
    pub path: Option<String>,
    /// Diff the index against HEAD.
    #[serde(default)]
    pub staged: Option<bool>,
    /// Optional base revision/range.
    #[serde(default)]
    pub base: Option<String>,
    /// Optional head revision.
    #[serde(default)]
    pub head: Option<String>,
    /// Include untracked files.
    #[serde(default)]
    pub include_untracked: Option<bool>,
    /// Unified diff context lines.
    #[serde(default)]
    pub context_lines: Option<u32>,
    /// Ignore whitespace-only changes.
    #[serde(default)]
    pub ignore_whitespace: Option<bool>,
}

/// Query for `GET /workspaces/{id}/git/log`.
#[derive(Debug, Deserialize, Default)]
pub struct LogQuery {
    /// Maximum number of commits.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Restrict history to a path.
    #[serde(default)]
    pub path: Option<String>,
    /// Branch/tag/commit to start from.
    #[serde(default)]
    pub reference: Option<String>,
}

/// `GET /api/v1/workspaces/{id}/git/status`
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<GitStatus>, AppError> {
    Ok(Json(GitService::status(&state, &id).await?))
}

/// `GET /api/v1/workspaces/{id}/git/branches`
pub async fn branches(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<GitBranch>>, AppError> {
    Ok(Json(GitService::branches(&state, &id).await?))
}

/// `GET /api/v1/workspaces/{id}/git/log`
pub async fn log(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<GitLogEntry>>, AppError> {
    let request = GitLogRpcRequest {
        workspace_id: id.clone(),
        log: GitLogRequest {
            limit: query.limit,
            path: query.path,
            reference: query.reference,
        },
    };
    Ok(Json(GitService::log(&state, &id, request).await?))
}

/// `GET /api/v1/workspaces/{id}/git/diff`
pub async fn diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<GitDiff>, AppError> {
    let request = diff_request(&id, query);
    Ok(Json(GitService::diff(&state, &id, request).await?))
}

/// `GET /api/v1/workspaces/{id}/git/diff/summary`
pub async fn diff_summary(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<GitDiffSummary>, AppError> {
    let request = diff_request(&id, query);
    Ok(Json(GitService::diff_summary(&state, &id, request).await?))
}

fn diff_request(workspace_id: &str, query: DiffQuery) -> GitDiffRequest {
    GitDiffRequest {
        workspace_id: workspace_id.to_string(),
        options: GitDiffOptions {
            path: query.path,
            staged: query.staged.unwrap_or(false),
            base: query.base,
            head: query.head,
            include_untracked: query.include_untracked.unwrap_or(false),
            context_lines: query.context_lines,
            ignore_whitespace: query.ignore_whitespace.unwrap_or(false),
        },
    }
}

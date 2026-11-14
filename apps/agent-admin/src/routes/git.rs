//! Git read endpoints, proxied to the owning machine.

use agent_fleet_auth::TokenAuth;
use agent_fleet_common::{error_codes, Ack};
use agent_fleet_protocol::git::{
    GitBranch, GitCheckoutOptions, GitCheckoutResult, GitCommitOptions, GitCommitResult, GitDiff,
    GitDiffOptions, GitDiffSummary, GitLogEntry, GitLogRequest, GitPathsRequest, GitStatus,
};
use agent_fleet_protocol::rpc::{GitDiffRequest, GitLogRpcRequest};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::acl;
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
    Extension(principal): Extension<Principal>,
) -> Result<Json<GitStatus>, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
    Ok(Json(GitService::status(&state, &id).await?))
}

/// `GET /api/v1/workspaces/{id}/git/branches`
pub async fn branches(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<GitBranch>>, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
    Ok(Json(GitService::branches(&state, &id).await?))
}

/// `GET /api/v1/workspaces/{id}/git/log`
pub async fn log(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<GitLogEntry>>, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
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
    Extension(principal): Extension<Principal>,
) -> Result<Json<GitDiff>, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
    let request = diff_request(&id, query);
    Ok(Json(GitService::diff(&state, &id, request).await?))
}

/// `GET /api/v1/workspaces/{id}/git/diff/summary`
pub async fn diff_summary(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DiffQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<GitDiffSummary>, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
    let request = diff_request(&id, query);
    Ok(Json(GitService::diff_summary(&state, &id, request).await?))
}

/// Header carrying the optional dedicated Git write token.
pub const GIT_WRITE_TOKEN_HEADER: &str = "x-agent-fleet-git-token";

/// `POST /api/v1/workspaces/{id}/git/stage`
pub async fn stage(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    Json(body): Json<GitPathsRequest>,
) -> Result<Json<Ack>, AppError> {
    authorize_write(&state, &principal, &headers, &id, "git.stage").await?;
    let org = authorize_workspace(&state, &principal, &id, "git.stage").await?;
    let result = GitService::stage(&state, &id, body.paths.clone()).await;
    finish_write(
        &state,
        &principal,
        "git.stage",
        &id,
        org.as_deref(),
        json!({"paths": body.paths}),
        result,
    )
    .await?;
    Ok(Json(Ack::ok()))
}

/// `POST /api/v1/workspaces/{id}/git/unstage`
pub async fn unstage(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    Json(body): Json<GitPathsRequest>,
) -> Result<Json<Ack>, AppError> {
    authorize_write(&state, &principal, &headers, &id, "git.unstage").await?;
    let org = authorize_workspace(&state, &principal, &id, "git.unstage").await?;
    let result = GitService::unstage(&state, &id, body.paths.clone()).await;
    finish_write(
        &state,
        &principal,
        "git.unstage",
        &id,
        org.as_deref(),
        json!({"paths": body.paths}),
        result,
    )
    .await?;
    Ok(Json(Ack::ok()))
}

/// `POST /api/v1/workspaces/{id}/git/commit`
pub async fn commit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    Json(body): Json<GitCommitOptions>,
) -> Result<Json<GitCommitResult>, AppError> {
    authorize_write(&state, &principal, &headers, &id, "git.commit").await?;
    let org = authorize_workspace(&state, &principal, &id, "git.commit").await?;
    let mut details = json!({
        "message": body.message.clone(),
        "paths": body.paths.clone(),
        "amend": body.amend,
    });
    match GitService::commit(&state, &id, body).await {
        Ok(commit) => {
            details["outcome"] = json!("ok");
            details["commit"] = json!(commit.commit);
            details["branch"] = json!(commit.branch);
            audit(
                &state,
                &principal,
                "git.commit",
                &id,
                org.as_deref(),
                details,
            )
            .await;
            Ok(Json(commit))
        }
        Err(error) => {
            audit_write_error(
                &state,
                &principal,
                "git.commit",
                &id,
                org.as_deref(),
                details,
                &error,
            )
            .await;
            Err(error)
        }
    }
}

/// `POST /api/v1/workspaces/{id}/git/checkout`
pub async fn checkout(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    Json(body): Json<GitCheckoutOptions>,
) -> Result<Json<GitCheckoutResult>, AppError> {
    authorize_write(&state, &principal, &headers, &id, "git.checkout").await?;
    let org = authorize_workspace(&state, &principal, &id, "git.checkout").await?;
    let details = json!({"branch": body.branch.clone(), "create": body.create});
    match GitService::checkout(&state, &id, body).await {
        Ok(checkout) => {
            let mut details = details;
            details["outcome"] = json!("ok");
            details["checked_out"] = json!(checkout.branch);
            audit(
                &state,
                &principal,
                "git.checkout",
                &id,
                org.as_deref(),
                details,
            )
            .await;
            Ok(Json(checkout))
        }
        Err(error) => {
            audit_write_error(
                &state,
                &principal,
                "git.checkout",
                &id,
                org.as_deref(),
                details,
                &error,
            )
            .await;
            Err(error)
        }
    }
}

/// Check workspace ownership for a write and return its organization. ACL
/// failures are audited while never exposing another organization's subject.
async fn authorize_workspace(
    state: &AppState,
    principal: &Principal,
    workspace_id: &str,
    action: &str,
) -> Result<Option<String>, AppError> {
    match acl::ensure_workspace_access(state, principal, workspace_id).await {
        Ok(workspace) => Ok(state
            .db
            .machine_org(&workspace.machine_id)
            .await
            .ok()
            .flatten()),
        Err(error) => {
            audit_write_error(
                state,
                principal,
                action,
                workspace_id,
                principal.org_id(),
                json!({}),
                &error,
            )
            .await;
            Err(error)
        }
    }
}

/// Enforce the write policy: the global switch and, when configured, the
/// dedicated write token. Denials are audited.
async fn authorize_write(
    state: &AppState,
    principal: &Principal,
    headers: &HeaderMap,
    workspace_id: &str,
    action: &str,
) -> Result<(), AppError> {
    let denial = if !state.config.git.allow_writes {
        Some(AppError::new(
            StatusCode::FORBIDDEN,
            error_codes::FORBIDDEN,
            "Git write operations are disabled",
        ))
    } else if let Some(expected) = state.config.git.write_token.as_deref() {
        let provided = headers
            .get(GIT_WRITE_TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        match TokenAuth::new(expected).verify_raw(provided) {
            Ok(()) => None,
            Err(_) => Some(AppError::unauthorized("missing or invalid git write token")),
        }
    } else {
        None
    };

    match denial {
        Some(error) => {
            audit(
                state,
                principal,
                action,
                workspace_id,
                principal.org_id(),
                json!({
                    "outcome": "denied",
                    "code": error.error.code,
                    "message": error.error.message,
                }),
            )
            .await;
            Err(error)
        }
        None => Ok(()),
    }
}

/// Record the outcome of a successful write attempt.
async fn finish_write<T>(
    state: &AppState,
    principal: &Principal,
    action: &str,
    workspace_id: &str,
    org_id: Option<&str>,
    details: serde_json::Value,
    result: Result<T, AppError>,
) -> Result<Option<T>, AppError> {
    match result {
        Ok(value) => {
            let mut details = details;
            if let serde_json::Value::Object(ref mut map) = details {
                map.insert("outcome".to_string(), json!("ok"));
            }
            audit(state, principal, action, workspace_id, org_id, details).await;
            Ok(Some(value))
        }
        Err(error) => {
            audit_write_error(
                state,
                principal,
                action,
                workspace_id,
                org_id,
                details,
                &error,
            )
            .await;
            Err(error)
        }
    }
}

/// Record a failed write attempt together with the request that caused it.
async fn audit_write_error(
    state: &AppState,
    principal: &Principal,
    action: &str,
    workspace_id: &str,
    org_id: Option<&str>,
    details: serde_json::Value,
    error: &AppError,
) {
    audit(
        state,
        principal,
        action,
        workspace_id,
        org_id,
        json!({
            "request": details,
            "outcome": "error",
            "code": error.error.code,
            "message": error.error.message,
        }),
    )
    .await;
}

/// Append an audit record, logging (but not failing the request) on error.
async fn audit(
    state: &AppState,
    principal: &Principal,
    action: &str,
    workspace_id: &str,
    org_id: Option<&str>,
    details: serde_json::Value,
) {
    if let Err(error) = state
        .db
        .append_audit_log(&principal.actor, org_id, action, workspace_id, details)
        .await
    {
        tracing::error!(
            action,
            workspace_id,
            error = %error,
            "failed to append git-write audit record"
        );
    }
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

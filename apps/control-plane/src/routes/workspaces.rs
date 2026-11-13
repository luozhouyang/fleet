//! Workspace, directory tree, file and search endpoints.

use agent_fleet_protocol::rpc::{
    FileReadRequest, FileReadResponse, FileSearchRequest, RegisterWorkspaceRpcRequest,
    TreeListRequest,
};
use agent_fleet_protocol::workspace::{
    DirectoryListing, FileContent, ListDirectoryRequest, ReadFileRequest, SearchRequest,
    SearchResponse, Workspace,
};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::error::ApiJson;
use crate::error::AppError;
use crate::services::workspace_service::WorkspaceService;
use crate::state::AppState;

/// Query for `GET /workspaces`.
#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    /// Restrict results to one machine.
    #[serde(default)]
    pub machine_id: Option<String>,
}

/// Query for `GET /workspaces/{id}/tree`.
#[derive(Debug, Deserialize, Default)]
pub struct TreeQuery {
    /// Workspace-relative directory path.
    #[serde(default)]
    pub path: String,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of entries.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Include entries normally hidden by ignore rules.
    #[serde(default)]
    pub show_ignored: Option<bool>,
}

/// Query for `GET /workspaces/{id}/files`.
#[derive(Debug, Deserialize, Default)]
pub struct FileQuery {
    /// Workspace-relative file path.
    #[serde(default)]
    pub path: String,
    /// Previously observed content ETag.
    #[serde(default)]
    pub etag: Option<String>,
    /// Maximum number of bytes to return.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

/// Query for `GET /workspaces/{id}/search`.
#[derive(Debug, Deserialize, Default)]
pub struct SearchQuery {
    /// Search text.
    #[serde(default)]
    pub q: String,
    /// Restrict the search to a subdirectory/file.
    #[serde(default)]
    pub path: Option<String>,
    /// Maximum number of matches.
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Match case exactly.
    #[serde(default)]
    pub case_sensitive: Option<bool>,
}

/// `GET /api/v1/workspaces?machine_id=`
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Workspace>>, AppError> {
    let machine_id = query.machine_id.as_deref().filter(|id| !id.is_empty());
    Ok(Json(WorkspaceService::list(&state, machine_id).await?))
}

/// `GET /api/v1/workspaces/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Workspace>, AppError> {
    Ok(Json(WorkspaceService::get(&state, &id).await?))
}

/// `POST /api/v1/workspaces`
pub async fn register(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<RegisterWorkspaceRpcRequest>,
) -> Result<Json<Workspace>, AppError> {
    Ok(Json(WorkspaceService::register(&state, request).await?))
}

/// `DELETE /api/v1/workspaces/{id}`
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    WorkspaceService::remove(&state, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/workspaces/{id}/tree`
pub async fn tree(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<DirectoryListing>, AppError> {
    let request = TreeListRequest {
        workspace_id: id.clone(),
        listing: ListDirectoryRequest {
            path: query.path,
            cursor: query.cursor,
            limit: query.limit,
            show_ignored: query.show_ignored,
        },
    };
    Ok(Json(WorkspaceService::tree(&state, &id, request).await?))
}

/// `GET /api/v1/workspaces/{id}/files`
pub async fn files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<FileQuery>,
) -> Result<Response, AppError> {
    let header_etag = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(normalize_etag);
    let etag = query
        .etag
        .clone()
        .map(|value| normalize_etag(&value))
        .or(header_etag);
    let conditional = etag.is_some();

    let read = FileReadRequest {
        workspace_id: id.clone(),
        read: ReadFileRequest {
            path: query.path.clone(),
            max_bytes: query.max_bytes,
            etag: etag.clone(),
        },
    };

    match WorkspaceService::read_file(&state, &id, read).await? {
        FileReadResponse::Content(content) => Ok(content_response(content)),
        FileReadResponse::NotModified(not_modified) => {
            let response_etag = if not_modified.etag.is_empty() {
                etag.unwrap_or_default()
            } else {
                not_modified.etag
            };
            if conditional {
                Ok(not_modified_response(&response_etag))
            } else {
                // An unconditional read should never be short-circuited. Retry
                // once without an ETag so the caller gets actual content.
                let retry = FileReadRequest {
                    workspace_id: id.clone(),
                    read: ReadFileRequest {
                        path: query.path,
                        max_bytes: query.max_bytes,
                        etag: None,
                    },
                };
                match WorkspaceService::read_file(&state, &id, retry).await? {
                    FileReadResponse::Content(content) => Ok(content_response(content)),
                    FileReadResponse::NotModified(not_modified) => {
                        Ok(not_modified_response(&not_modified.etag))
                    }
                }
            }
        }
    }
}

/// `GET /api/v1/workspaces/{id}/search`
pub async fn search(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, AppError> {
    let request = FileSearchRequest {
        workspace_id: id.clone(),
        search: SearchRequest {
            query: query.q,
            path: query.path,
            max_results: query.max_results,
            case_sensitive: query.case_sensitive,
        },
    };
    Ok(Json(WorkspaceService::search(&state, &id, request).await?))
}

fn content_response(content: FileContent) -> Response {
    let etag = content.etag.clone();
    let mut response = Json(content).into_response();
    insert_etag(&mut response, &etag);
    response
}

fn not_modified_response(etag: &str) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    insert_etag(&mut response, etag);
    response
}

fn insert_etag(response: &mut Response, etag: &str) {
    if etag.is_empty() {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
}

/// Strip weak markers and quotes from an `If-None-Match` value.
fn normalize_etag(value: &str) -> String {
    value
        .trim()
        .strip_prefix("W/")
        .unwrap_or(value.trim())
        .trim_matches('"')
        .to_string()
}

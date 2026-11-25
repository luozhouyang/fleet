//! Workspace, directory tree, file and search endpoints.

use std::io;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use base64::Engine;
use fleet_core_protocol::rpc::{
    FileReadRequest, FileReadResponse, FileSearchRequest, RegisterWorkspaceRpcRequest,
    TreeListRequest,
};
use fleet_core_protocol::workspace::{
    DirectoryListing, FileContent, ListDirectoryRequest, ReadFileRangeRequest, ReadFileRequest,
    SearchRequest, SearchResponse, Workspace, DEFAULT_FILE_RANGE_BYTES,
};
use futures::stream;
use serde::Deserialize;

use crate::error::ApiJson;
use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::acl;
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

/// Query for `GET /workspaces/{id}/files/raw`.
#[derive(Debug, Deserialize, Default)]
pub struct RawFileQuery {
    /// Workspace-relative file path.
    #[serde(default)]
    pub path: String,
    /// Add a `Content-Disposition: attachment` header.
    #[serde(default)]
    pub download: Option<bool>,
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
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<Workspace>>, AppError> {
    let machine_id = query.machine_id.as_deref().filter(|id| !id.is_empty());
    if let Some(machine_id) = machine_id {
        acl::ensure_machine_access(&state, &principal, machine_id).await?;
    }
    let mut workspaces = WorkspaceService::list(&state, machine_id).await?;
    if let Some(visible) = acl::visible_machine_ids(&state, &principal).await? {
        workspaces.retain(|workspace| visible.contains(&workspace.machine_id));
    }
    Ok(Json(workspaces))
}

/// `GET /api/v1/workspaces/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Workspace>, AppError> {
    Ok(Json(
        acl::ensure_workspace_access(&state, &principal, &id).await?,
    ))
}

/// `POST /api/v1/workspaces`
pub async fn register(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<RegisterWorkspaceRpcRequest>,
) -> Result<Json<Workspace>, AppError> {
    acl::ensure_machine_access(&state, &principal, &request.machine_id).await?;
    Ok(Json(WorkspaceService::register(&state, request).await?))
}

/// `DELETE /api/v1/workspaces/{id}`
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
    WorkspaceService::remove(&state, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/workspaces/{id}/tree`
pub async fn tree(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TreeQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<DirectoryListing>, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
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
    Extension(principal): Extension<Principal>,
) -> Result<Response, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
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
    Extension(principal): Extension<Principal>,
) -> Result<Json<SearchResponse>, AppError> {
    acl::ensure_workspace_access(&state, &principal, &id).await?;
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

/// `GET /api/v1/workspaces/{id}/files/raw`
///
/// Streams the file with HTTP range support. The body is pulled from the
/// daemon in 64 KiB chunks; the next chunk is requested only after the
/// previous one has been accepted by the HTTP body, so a slow or disconnected
/// client applies backpressure and cancels the remaining reads.
pub async fn raw_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<RawFileQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, AppError> {
    let workspace = acl::ensure_workspace_access(&state, &principal, &id).await?;
    if query.path.trim().is_empty() {
        return Err(AppError::bad_request("path is required"));
    }

    let metadata = WorkspaceService::stat_file(&state, &workspace.id, &query.path).await?;
    let size = metadata.size;
    let etag = metadata.etag.clone();
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    // Conditional requests apply to full reads only; a range is always
    // served (If-Range is not supported).
    if range_header.is_none()
        && headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| etag_matches(value, &etag))
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        insert_etag(&mut response, &etag);
        return Ok(response);
    }

    let (start, end) = match range_header.as_deref() {
        Some(value) => match parse_range(value, size)? {
            Some(range) => range,
            None => return Ok(range_not_satisfiable(size, &etag)),
        },
        None => (0, size),
    };
    let partial = range_header.is_some();
    let length = end - start;

    let mut builder = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length)
        .header(
            header::CONTENT_TYPE,
            if metadata.is_binary {
                "application/octet-stream"
            } else {
                "text/plain; charset=utf-8"
            },
        )
        .header(header::ETAG, etag.clone());
    if partial {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end.saturating_sub(1), size),
        );
    }
    if query.download.unwrap_or(false) {
        builder = builder.header(header::CONTENT_DISPOSITION, attachment(&query.path));
    }
    builder
        .body(Body::from_stream(range_stream(
            state,
            workspace.machine_id,
            workspace.id,
            query.path,
            start,
            end,
            etag,
            size,
        )))
        .map_err(AppError::internal)
}

#[allow(clippy::too_many_arguments)]
fn range_stream(
    state: AppState,
    machine_id: String,
    workspace_id: String,
    path: String,
    start: u64,
    end: u64,
    etag: String,
    total_size: u64,
) -> impl stream::Stream<Item = Result<Bytes, io::Error>> {
    stream::unfold(
        Some((
            state,
            machine_id,
            workspace_id,
            path,
            start,
            end,
            etag,
            total_size,
        )),
        |source| async move {
            let (state, machine_id, workspace_id, path, offset, end, etag, total_size) = source?;
            if offset >= end {
                return None;
            }
            let length = (end - offset).min(DEFAULT_FILE_RANGE_BYTES);
            let request = ReadFileRangeRequest {
                path: path.clone(),
                offset,
                length: Some(length),
            };
            match WorkspaceService::read_file_range_on(&state, &workspace_id, &machine_id, request)
                .await
            {
                Ok(range) => {
                    if range.etag != etag || range.total_size != total_size {
                        tracing::warn!(
                            workspace = %workspace_id,
                            path = %path,
                            offset,
                            "file changed during the download"
                        );
                        return Some((
                            Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "file changed during the download",
                            )),
                            None,
                        ));
                    }
                    let bytes = match base64::engine::general_purpose::STANDARD.decode(&range.data)
                    {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            tracing::warn!(
                                workspace = %workspace_id,
                                path = %path,
                                error = %error,
                                "invalid base64 in a file range"
                            );
                            return Some((
                                Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "invalid file range",
                                )),
                                None,
                            ));
                        }
                    };
                    let next_offset = offset + bytes.len() as u64;
                    let next = if bytes.is_empty() || next_offset >= end {
                        None
                    } else {
                        Some((
                            state,
                            machine_id,
                            workspace_id,
                            path,
                            next_offset,
                            end,
                            etag,
                            total_size,
                        ))
                    };
                    Some((Ok(Bytes::from(bytes)), next))
                }
                Err(error) => {
                    tracing::warn!(
                        workspace = %workspace_id,
                        path = %path,
                        offset,
                        code = %error.error.code,
                        error = %error.error.message,
                        "raw file read failed"
                    );
                    Some((Err(io::Error::other("workspace file read failed")), None))
                }
            }
        },
    )
}

/// Parse one `bytes=` range. `Ok(None)` means the range is unsatisfiable.
fn parse_range(value: &str, size: u64) -> Result<Option<(u64, u64)>, AppError> {
    let spec = value
        .trim()
        .strip_prefix("bytes=")
        .ok_or_else(|| AppError::bad_request("only `bytes` ranges are supported"))?;
    if spec.contains(',') {
        return Err(AppError::bad_request(
            "multiple byte ranges are not supported",
        ));
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Err(AppError::bad_request("invalid range header"));
    };
    if size == 0 {
        return Ok(None);
    }
    if first.trim().is_empty() {
        let suffix: u64 = last
            .trim()
            .parse()
            .map_err(|_| AppError::bad_request("invalid range suffix"))?;
        if suffix == 0 {
            return Ok(None);
        }
        return Ok(Some((size.saturating_sub(suffix), size)));
    }
    let start: u64 = first
        .trim()
        .parse()
        .map_err(|_| AppError::bad_request("invalid range start"))?;
    if start >= size {
        return Ok(None);
    }
    let end = if last.trim().is_empty() {
        size
    } else {
        let inclusive: u64 = last
            .trim()
            .parse()
            .map_err(|_| AppError::bad_request("invalid range end"))?;
        if inclusive < start {
            return Err(AppError::bad_request("range end is before its start"));
        }
        inclusive.saturating_add(1).min(size)
    };
    Ok(Some((start, end)))
}

fn range_not_satisfiable(size: u64, etag: &str) -> Response {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    let value = HeaderValue::from_str(&format!("bytes */{size}"))
        .unwrap_or_else(|_| HeaderValue::from_static("bytes */*"));
    response.headers_mut().insert(header::CONTENT_RANGE, value);
    insert_etag(&mut response, etag);
    response
}

fn etag_matches(candidate: &str, etag: &str) -> bool {
    let candidate = candidate.trim();
    candidate == "*"
        || candidate
            .split(',')
            .any(|value| normalize_etag(value) == normalize_etag(etag))
}

fn attachment(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("file");
    let fallback: String = name
        .chars()
        .map(|character| {
            if character.is_ascii()
                && !character.is_control()
                && character != '"'
                && character != '\\'
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    let fallback = if fallback.trim_matches('_').is_empty() {
        "file"
    } else {
        &fallback
    };
    if name.is_ascii() {
        return format!("attachment; filename=\"{fallback}\"");
    }
    // RFC 5987 lets the browser keep the original name; the ASCII filename
    // stays as the fallback for clients that ignore `filename*`.
    format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{}",
        rfc5987(name)
    )
}

/// Percent-encode a value for the RFC 5987 `filename*` parameter.
fn rfc5987(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            )
        {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
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

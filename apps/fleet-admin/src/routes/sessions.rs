//! Agent session endpoints.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Extension;
use axum::Json;
use fleet_core_protocol::agent::{
    AgentPermissionRequest, AgentQuestionRequest, AgentSession, SendMessageRequest,
    UpdateSessionRequest,
};
use fleet_core_protocol::rpc::{CreateSessionRpcRequest, SessionDetail};

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::Principal;
use crate::routes::idempotency::idempotency_key;
use crate::services::acl;
use crate::services::agent_service::AgentService;
use crate::services::session_query::{self, SessionFilter, SessionSort};
use crate::services::session_ui_service::{self, actor_id};
use crate::state::AppState;
use serde::Deserialize;

/// Query for `GET /sessions`.
///
/// Without any of the rail parameters the response stays a bare array, so
/// existing clients keep working. Asking for any of them opts into the paged
/// envelope.
#[derive(Debug, Deserialize, Default)]
pub struct ListSessionsQuery {
    /// Include archived (`terminated`) sessions.
    #[serde(default)]
    pub include_closed: Option<bool>,
    /// Include sessions the caller archived.
    #[serde(default)]
    pub include_archived: Option<bool>,
    /// Comma-separated status names (`running,waiting_for_permission,...`).
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub unread: Option<bool>,
    /// Comma-separated labels; a session must carry all of them.
    #[serde(default)]
    pub labels: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

impl ListSessionsQuery {
    /// Whether the caller asked for the paged rail shape.
    fn is_paged(&self) -> bool {
        self.include_archived.is_some()
            || self.status.is_some()
            || self.machine_id.is_some()
            || self.workspace_id.is_some()
            || self.pinned.is_some()
            || self.unread.is_some()
            || self.labels.is_some()
            || self.q.is_some()
            || self.sort.is_some()
            || self.limit.is_some()
            || self.cursor.is_some()
    }

    fn sort(&self) -> Result<SessionSort, AppError> {
        SessionSort::parse(self.sort.as_deref().unwrap_or("")).ok_or_else(|| {
            AppError::bad_request("sort must be one of `recent`, `created` or `title`")
        })
    }

    fn filter(&self) -> SessionFilter {
        let statuses = self
            .status
            .as_deref()
            .map(|status| {
                status
                    .split(',')
                    .map(str::trim)
                    .filter(|status| !status.is_empty())
                    .filter_map(|status| {
                        serde_json::from_value::<fleet_core_protocol::agent::SessionStatus>(
                            serde_json::Value::String(status.to_string()),
                        )
                        .ok()
                    })
                    .collect()
            })
            .unwrap_or_default();
        SessionFilter {
            statuses,
            archived: Some(self.include_archived.unwrap_or(false)),
            pinned: self.pinned,
            unread: self.unread,
            machine_id: self.machine_id.clone(),
            workspace_id: self.workspace_id.clone(),
            query: self.q.clone(),
            labels: self
                .labels
                .as_deref()
                .map(|labels| labels.split(',').map(str::to_string).collect())
                .unwrap_or_default(),
        }
    }
}

/// `GET /api/v1/sessions`
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    let sort = query.sort()?;
    let paged = query.is_paged();
    let mut sessions =
        AgentService::list_sessions(&state, query.include_closed.unwrap_or(false)).await?;
    if let Some(visible) = acl::visible_machine_ids(&state, &principal).await? {
        sessions.retain(|session| visible.contains(&session.machine_id));
    }
    // Attach the caller's pins, archive, labels and unread before filtering, so
    // every filter sees the values it selects on.
    session_ui_service::overlay(&state, &actor_id(&principal), &mut sessions).await;

    if !paged {
        return Ok(Json(serde_json::json!(sessions)));
    }

    let mut sessions = session_query::filter_sessions(&sessions, &query.filter());
    session_query::sort_sessions(&mut sessions, sort);
    let page = session_query::paginate(
        sessions,
        query.cursor.as_deref(),
        session_ui_service::clamp_limit(query.limit),
        sort,
    );
    Ok(Json(serde_json::to_value(page).unwrap_or_default()))
}

/// `GET /api/v1/sessions/facets`
///
/// Counts over the caller's visible sessions, respecting `include_closed` but
/// no other filter: the chips must not shrink as the user narrows the list.
pub async fn facets(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<session_query::SessionFacets>, AppError> {
    let mut sessions =
        AgentService::list_sessions(&state, query.include_closed.unwrap_or(true)).await?;
    if let Some(visible) = acl::visible_machine_ids(&state, &principal).await? {
        sessions.retain(|session| visible.contains(&session.machine_id));
    }
    session_ui_service::overlay(&state, &actor_id(&principal), &mut sessions).await;
    Ok(Json(session_query::facets(&sessions)))
}

/// `POST /api/v1/sessions`
pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<CreateSessionRpcRequest>,
) -> Result<Json<AgentSession>, AppError> {
    let key = idempotency_key(&headers)?;
    acl::ensure_machine_access(&state, &principal, &request.machine_id).await?;
    if let Some(workspace_id) = request.create.workspace_id.as_deref() {
        acl::ensure_workspace_access(&state, &principal, workspace_id).await?;
    }
    Ok(Json(
        AgentService::create_session(&state, request, key.as_deref()).await?,
    ))
}

/// `GET /api/v1/sessions/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SessionDetail>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    let mut detail = AgentService::get_session(&state, &id).await?;
    let actor = actor_id(&principal);
    let mut sessions = vec![detail.session];
    session_ui_service::overlay(&state, &actor, &mut sessions).await;
    detail.session = sessions.pop().expect("one session");
    Ok(Json(detail))
}

/// `PATCH /api/v1/sessions/{id}` — update mutable session metadata.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<UpdateSessionRequest>,
) -> Result<Json<AgentSession>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(
        AgentService::update_session(&state, &id, request).await?,
    ))
}

/// `POST /api/v1/sessions/{id}/close`
pub async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = idempotency_key(&headers)?;
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(
        AgentService::close(&state, &id, key.as_deref()).await?,
    ))
}

/// `DELETE /api/v1/sessions/{id}` — permanently removes history.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::delete(&state, &id).await?))
}

/// `POST /api/v1/sessions/{id}/messages`
pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    ApiJson(message): ApiJson<SendMessageRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = idempotency_key(&headers)?;
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(
        AgentService::send_message(&state, &id, message, key.as_deref()).await?,
    ))
}

/// `POST /api/v1/sessions/{id}/interrupt`
pub async fn interrupt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = idempotency_key(&headers)?;
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(
        AgentService::interrupt(&state, &id, key.as_deref()).await?,
    ))
}

/// `POST /api/v1/sessions/{id}/resume`
pub async fn resume(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::resume(&state, &id).await?))
}

/// `GET /api/v1/sessions/{id}/permissions`
pub async fn permissions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<AgentPermissionRequest>>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::list_permissions(&state, &id).await?))
}

pub async fn questions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<AgentQuestionRequest>>, AppError> {
    acl::ensure_session_access(&state, &principal, &id).await?;
    Ok(Json(AgentService::list_questions(&state, &id).await?))
}

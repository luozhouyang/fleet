//! User and token administration endpoints.

use agent_fleet_auth::{generate_user_token, hash_token};
use agent_fleet_protocol::authz::{CreatedUser, RotatedUserToken, User};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::{Principal, PrincipalKind};
use crate::services::acl;
use crate::state::AppState;

/// Query for `GET /api/v1/users`.
#[derive(Debug, Deserialize, Default)]
pub struct ListUsersQuery {
    /// Restrict to one organization. Ignored for organization-scoped users.
    #[serde(default)]
    pub org_id: Option<String>,
}

/// `GET /api/v1/users` – administrators only.
pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    axum::extract::Query(query): axum::extract::Query<ListUsersQuery>,
) -> Result<Json<Vec<User>>, AppError> {
    acl::ensure_admin(&principal)?;
    let org_filter = match principal.org_id() {
        Some(org_id) => Some(org_id.to_string()),
        None => query.org_id.filter(|id| !id.is_empty()),
    };
    Ok(Json(state.db.list_users(org_filter.as_deref()).await?))
}

/// `POST /api/v1/users` – create a user and return its bearer token once.
pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<agent_fleet_protocol::authz::CreateUserRequest>,
) -> Result<Json<CreatedUser>, AppError> {
    acl::ensure_admin(&principal)?;

    let name = request.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("user name must not be empty"));
    }
    if name.len() > 120 {
        return Err(AppError::bad_request(
            "user name is limited to 120 characters",
        ));
    }
    let organization = state
        .db
        .get_organization(&request.org_id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("organization {} not found", request.org_id)))?;
    if !principal.can_access_org(Some(&organization.id)) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            agent_fleet_common::error_codes::FORBIDDEN,
            "cannot create users outside your organization",
        ));
    }

    let token = generate_user_token();
    let user = state
        .db
        .create_user(&organization.id, name, request.role, &hash_token(&token))
        .await?;
    audit_user(
        &state,
        &principal,
        "user.create",
        &user,
        json!({"role": user.role.as_str()}),
    )
    .await;
    Ok(Json(CreatedUser { user, token }))
}

/// `DELETE /api/v1/users/{id}` – administrators only; users cannot delete
/// themselves through this endpoint.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    acl::ensure_admin(&principal)?;
    if principal_user_id(&principal) == Some(id.as_str()) {
        return Err(AppError::bad_request(
            "administrators cannot delete themselves",
        ));
    }
    let user = state
        .db
        .get_user(&id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("user {id} not found")))?;
    if !principal.can_access_org(Some(&user.org_id)) {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    if !state.db.delete_user(&id).await? {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    audit_user(&state, &principal, "user.delete", &user, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/users/{id}/rotate-token`
///
/// A user may rotate their own token; administrators may rotate tokens of
/// users in their organization. The previous token stops working immediately.
pub async fn rotate_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<RotatedUserToken>, AppError> {
    let user = state
        .db
        .get_user(&id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("user {id} not found")))?;
    let is_self = principal_user_id(&principal) == Some(user.id.as_str());
    if !is_self {
        acl::ensure_admin(&principal)?;
    }
    if !principal.can_access_org(Some(&user.org_id)) {
        return Err(AppError::not_found(format!("user {id} not found")));
    }

    let token = generate_user_token();
    if !state
        .db
        .update_user_token_hash(&user.id, &hash_token(&token))
        .await?
    {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    audit_user(&state, &principal, "user.rotate_token", &user, json!({})).await;
    Ok(Json(RotatedUserToken {
        user_id: user.id,
        token,
    }))
}

fn principal_user_id(principal: &Principal) -> Option<&str> {
    match &principal.kind {
        PrincipalKind::LegacyAdmin => None,
        PrincipalKind::User(user) => Some(user.id.as_str()),
    }
}

async fn audit_user(
    state: &AppState,
    principal: &Principal,
    action: &str,
    user: &User,
    details: serde_json::Value,
) {
    if let Err(error) = state
        .db
        .append_audit_log(&principal.actor, action, &user.id, details)
        .await
    {
        tracing::warn!(error = %error, action, "failed to audit user action");
    }
}

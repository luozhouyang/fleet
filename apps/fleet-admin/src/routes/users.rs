//! User and token administration endpoints.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use fleet_core_auth::{generate_user_token, hash_token};
use fleet_core_common::{now, Timestamp};
use fleet_core_protocol::authz::{
    CreateUserRequest, CreatedUser, Me, PrincipalKind as AuthPrincipalKind, RotatedUserToken, User,
    UserRole,
};
use serde::Deserialize;
use serde_json::json;

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::{Principal, PrincipalKind};
use crate::services::acl;
use crate::state::AppState;

/// Hard upper bound for per-request token lifetimes.
const MAX_USER_TOKEN_TTL_DAYS: u64 = 3650;

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
    Ok(Json(state.db.users.list(org_filter.as_deref()).await?))
}

/// `GET /api/v1/me` – describe the authenticated caller.
pub async fn me(Extension(principal): Extension<Principal>) -> Json<Me> {
    match &principal.kind {
        PrincipalKind::LegacyAdmin => Json(Me {
            kind: AuthPrincipalKind::LegacyAdmin,
            user: None,
            org_id: None,
            role: UserRole::Admin,
        }),
        PrincipalKind::User(user) => Json(Me {
            kind: AuthPrincipalKind::User,
            user: Some(user.clone()),
            org_id: Some(user.org_id.clone()),
            role: user.role,
        }),
    }
}

/// Query for `POST /api/v1/users/{id}/rotate-token`.
#[derive(Debug, Deserialize, Default)]
pub struct RotateTokenQuery {
    /// Optional lifetime override in days; `0` means never expires.
    #[serde(default)]
    pub expires_in_days: Option<u64>,
}

/// Resolve an explicit token lifetime against the configured default.
fn resolve_token_ttl_days(state: &AppState, requested: Option<u64>) -> Result<u64, AppError> {
    match requested {
        Some(days) if days > MAX_USER_TOKEN_TTL_DAYS => Err(AppError::bad_request(format!(
            "token expiry is limited to {MAX_USER_TOKEN_TTL_DAYS} days"
        ))),
        Some(days) => Ok(days),
        None => Ok(state.config.auth.user_token_ttl_days),
    }
}

/// Convert a lifetime in days into an absolute expiry. `0` means never.
fn token_expiry(issued_at: Timestamp, ttl_days: u64) -> Option<Timestamp> {
    if ttl_days == 0 {
        return None;
    }
    let days = i64::try_from(ttl_days).unwrap_or(i64::MAX);
    let Some(duration) = chrono::Duration::try_days(days) else {
        return Some(chrono::DateTime::<chrono::Utc>::MAX_UTC);
    };
    Some(
        issued_at
            .checked_add_signed(duration)
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC),
    )
}

/// `POST /api/v1/users` – create a user and return its bearer token once.
pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateUserRequest>,
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
        .organizations
        .get(&request.org_id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("organization {} not found", request.org_id)))?;
    if !principal.can_access_org(Some(&organization.id)) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            fleet_core_common::error_codes::FORBIDDEN,
            "cannot create users outside your organization",
        ));
    }

    let ttl_days = resolve_token_ttl_days(&state, request.token_expires_in_days)?;
    let issued_at = now();
    let expires_at = token_expiry(issued_at, ttl_days);
    let token = generate_user_token();
    let user = state
        .db
        .users
        .create(
            &organization.id,
            name,
            request.role,
            &hash_token(&token),
            issued_at,
            expires_at,
        )
        .await?;
    audit_user(
        &state,
        &principal,
        "user.create",
        &user,
        json!({
            "role": user.role.as_str(),
            "token_expires_at": user.token_expires_at.map(|value| value.to_rfc3339()),
        }),
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
        .users
        .get(&id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("user {id} not found")))?;
    if !principal.can_access_org(Some(&user.org_id)) {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    if !state.db.users.delete(&id).await? {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    audit_user(&state, &principal, "user.delete", &user, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/users/{id}/rotate-token`
///
/// A user may rotate their own token; administrators may rotate tokens of
/// users in their organization. The previous token stops working immediately.
/// An optional `expires_in_days` query parameter overrides the configured
/// default lifetime; omitting it keeps the empty-body request compatible.
pub async fn rotate_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RotateTokenQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<RotatedUserToken>, AppError> {
    let user = state
        .db
        .users
        .get(&id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("user {id} not found")))?;
    let is_self = principal_user_id(&principal) == Some(user.id.as_str());
    if !is_self {
        acl::ensure_admin(&principal)?;
    }
    if !principal.can_access_org(Some(&user.org_id)) {
        return Err(AppError::not_found(format!("user {id} not found")));
    }

    let ttl_days = resolve_token_ttl_days(&state, query.expires_in_days)?;
    let issued_at = now();
    let expires_at = token_expiry(issued_at, ttl_days);
    let token = generate_user_token();
    if !state
        .db
        .users
        .update_token(&user.id, &hash_token(&token), issued_at, expires_at)
        .await?
    {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    audit_user(
        &state,
        &principal,
        "user.rotate_token",
        &user,
        json!({"token_expires_at": expires_at.map(|value| value.to_rfc3339())}),
    )
    .await;
    Ok(Json(RotatedUserToken {
        user_id: user.id,
        token,
        token_expires_at: expires_at,
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
        .audit
        .append(
            &principal.actor,
            Some(&user.org_id),
            action,
            &user.id,
            details,
        )
        .await
    {
        tracing::warn!(error = %error, action, "failed to audit user action");
    }
}

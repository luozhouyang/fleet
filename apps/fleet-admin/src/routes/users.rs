//! User, membership and token administration endpoints.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use fleet_admin_storage::auth_identities::UnlinkIdentity;
use fleet_core_auth::{generate_user_token, hash_token};
use fleet_core_common::{now, Timestamp};
use fleet_core_protocol::authz::{
    CreateUserRequest, CreatedUser, LinkedIdentity, Me, PrincipalKind as AuthPrincipalKind,
    RotatedUserToken, User, UserRole,
};
use serde::Deserialize;
use serde_json::json;

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::{Principal, PrincipalKind};
use crate::services::acl;
use crate::services::org_service::OrgService;
use crate::services::policy::{Action, Policy};
use crate::state::AppState;

/// Hard upper bound for per-request token lifetimes.
const MAX_USER_TOKEN_TTL_DAYS: u64 = 3650;

/// Query for `GET /api/v1/users`.
#[derive(Debug, Deserialize, Default)]
pub struct ListUsersQuery {
    /// Restrict to members of this organization. Defaults to the active one.
    #[serde(default)]
    pub org_id: Option<String>,
}

/// `GET /api/v1/users` – members of the active organization.
pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<Vec<User>>, AppError> {
    if principal.is_legacy_admin() {
        return Ok(Json(state.db.users.list().await?));
    }
    let org_id = query
        .org_id
        .filter(|id| !id.is_empty())
        .or_else(|| principal.org_id().map(str::to_owned))
        .ok_or_else(|| AppError::forbidden("no active organization"))?;
    // The role must be checked in the organization being read, not only in
    // the caller's active one; `org_id` is caller-controlled.
    let caller_role = acl::ensure_org_member(&principal, &org_id)?;
    if !Policy::allows(caller_role, Action::ManageMembers) {
        return Err(AppError::forbidden(
            "you may not list users in this organization",
        ));
    }
    if state
        .db
        .organizations
        .get(&org_id)
        .await?
        .is_none_or(|organization| organization.deleted_at.is_some())
    {
        return Err(AppError::not_found(format!(
            "organization {org_id} not found"
        )));
    }
    let members = state.db.memberships.members_of(&org_id).await?;
    let mut users = Vec::with_capacity(members.len());
    for member in members {
        if let Some(user) = state.db.users.get(&member.user_id).await? {
            users.push(user);
        }
    }
    Ok(Json(users))
}

/// `GET /api/v1/me` – describe the authenticated caller.
pub async fn me(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Me>, AppError> {
    if principal.is_legacy_admin() {
        return Ok(Json(Me {
            kind: AuthPrincipalKind::LegacyAdmin,
            user: None,
            org_id: None,
            role: UserRole::Owner,
        }));
    }
    let user = principal
        .user()
        .cloned()
        .ok_or_else(|| AppError::unauthorized("no user identity"))?;
    // The organization is eagerly created on first sign-in, but an account
    // created by an administrator may hit `/me` before that; creating it here
    // keeps the UI from rendering an empty organization switcher.
    let (org_id, role) = match principal.org_id() {
        Some(org_id) => (org_id.to_string(), principal.role),
        None => {
            let organization = OrgService::ensure_personal(&state, &user.id, &user.name).await?;
            (organization.id, UserRole::Owner)
        }
    };
    Ok(Json(Me {
        kind: AuthPrincipalKind::User,
        user: Some(user),
        org_id: Some(org_id),
        role,
    }))
}

/// `GET /api/v1/me/identities` – sign-in identities linked to the caller.
pub async fn identities(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<LinkedIdentity>>, AppError> {
    let user = principal
        .user()
        .ok_or_else(|| AppError::forbidden("the bootstrap token has no linked identities"))?;
    let identities = state.db.auth_identities.for_user(&user.id).await?;
    Ok(Json(
        identities
            .into_iter()
            .map(|identity| LinkedIdentity {
                id: identity.id,
                provider: identity.provider,
                email: identity.email,
                login: identity.login,
                avatar_url: identity.avatar_url,
                created_at: identity.created_at,
                last_login_at: identity.last_login_at,
            })
            .collect(),
    ))
}

/// `DELETE /api/v1/me/identities/{id}` – unlink one sign-in identity.
pub async fn unlink_identity(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(identity_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let user = principal
        .user()
        .ok_or_else(|| AppError::forbidden("the bootstrap token has no linked identities"))?;
    match state
        .db
        .auth_identities
        .unlink(&user.id, &identity_id)
        .await?
    {
        UnlinkIdentity::Unlinked { provider } => {
            audit_identity_unlink(&state, &principal, &user.id, &provider).await;
            Ok(StatusCode::NO_CONTENT)
        }
        UnlinkIdentity::NotFound => Err(AppError::not_found("identity not found")),
        UnlinkIdentity::LastSignInMethod => Err(AppError::conflict(
            "this is the last way to sign in; keep a user token or link another identity first",
        )),
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

/// `POST /api/v1/users` – create an account and return its bearer token once.
///
/// The account is global and the membership is created in the target
/// organization, so the CLI can create a second administrator without OAuth.
pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateUserRequest>,
) -> Result<Json<CreatedUser>, AppError> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("user name must not be empty"));
    }
    if name.len() > 120 {
        return Err(AppError::bad_request(
            "user name is limited to 120 characters",
        ));
    }
    let email = request.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(AppError::bad_request("a valid email address is required"));
    }
    if state.db.users.by_email(&email).await?.is_some() {
        return Err(AppError::conflict(format!(
            "an account for {email} already exists"
        )));
    }

    let org_id = request
        .org_id
        .clone()
        .or_else(|| principal.org_id().map(str::to_owned))
        .ok_or_else(|| AppError::bad_request("org_id is required"))?;
    let caller_role = acl::ensure_org_member(&principal, &org_id)?;
    if state
        .db
        .organizations
        .get(&org_id)
        .await?
        .is_none_or(|organization| organization.deleted_at.is_some())
    {
        return Err(AppError::not_found(format!(
            "organization {org_id} not found"
        )));
    }
    if !caller_role.can_assign(request.role) {
        return Err(AppError::forbidden(format!(
            "role '{}' may not grant '{}'",
            caller_role.as_str(),
            request.role.as_str()
        )));
    }

    let ttl_days = resolve_token_ttl_days(&state, request.token_expires_in_days)?;
    let issued_at = now();
    let expires_at = token_expiry(issued_at, ttl_days);
    let token = generate_user_token();
    let user = state
        .db
        .users
        .create(
            Some(&email),
            name,
            None,
            Some(&hash_token(&token)),
            Some(issued_at),
            expires_at,
        )
        .await?;
    let membership = state
        .db
        .memberships
        .upsert(
            &org_id,
            &user.id,
            request.role,
            principal.user().map(|u| u.id.as_str()),
        )
        .await?;
    // Every account has a personal organization; create it eagerly so the
    // organization switcher is never empty for a freshly created account.
    OrgService::ensure_personal(&state, &user.id, &user.name).await?;
    audit_user(
        &state,
        &principal,
        Some(&org_id),
        "user.create",
        &user,
        json!({
            "role": request.role.as_str(),
            "token_expires_at": user.token_expires_at.map(|value| value.to_rfc3339()),
        }),
    )
    .await;
    Ok(Json(CreatedUser {
        user,
        membership,
        token,
    }))
}

/// `DELETE /api/v1/users/{id}` – administrators only; users cannot delete
/// themselves through this endpoint.
///
/// Deleting an account removes identities, sessions, memberships and pending
/// invitations. Machines, workspaces and audit rows are untouched.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    acl::ensure_action(&principal, Action::ManageMembers)?;
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
    if !principal.is_legacy_admin() && !shares_managed_organization(&state, &principal, &id).await?
    {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    if !state.db.users.delete(&id).await? {
        return Err(AppError::not_found(format!("user {id} not found")));
    }
    audit_user(
        &state,
        &principal,
        principal.org_id(),
        "user.delete",
        &user,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/users/{id}/disable`
///
/// Administrators only. Disabling blocks the bearer token and revokes every
/// browser session; it does not delete the account or stop running agents.
pub async fn disable(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<User>, AppError> {
    set_disabled(&state, &principal, &id, true).await
}

/// `POST /api/v1/users/{id}/enable`
///
/// Restores a disabled account. The previous bearer token is valid again;
/// rotate it first if it may have leaked while the account was disabled.
pub async fn enable(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<User>, AppError> {
    set_disabled(&state, &principal, &id, false).await
}

async fn set_disabled(
    state: &AppState,
    principal: &Principal,
    id: &str,
    disabled: bool,
) -> Result<Json<User>, AppError> {
    acl::ensure_action(principal, Action::ManageMembers)?;
    if principal_user_id(principal) == Some(id) {
        return Err(AppError::bad_request(
            "administrators cannot change their own account state",
        ));
    }
    let user = state
        .db
        .users
        .get(id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("user {id} not found")))?;
    if !principal.is_legacy_admin() && !shares_managed_organization(state, principal, id).await? {
        return Err(AppError::not_found(format!("user {id} not found")));
    }

    if disabled {
        if user.disabled_at.is_none() {
            if !state.db.users.disable(id).await? {
                return Err(AppError::not_found(format!("user {id} not found")));
            }
            let revoked = state.db.auth_sessions.revoke_all_for_user(id).await?;
            audit_user(
                state,
                principal,
                principal.org_id(),
                "user.disable",
                &user,
                json!({"sessions_revoked": revoked}),
            )
            .await;
        }
    } else if user.disabled_at.is_some() {
        if !state.db.users.enable(id).await? {
            return Err(AppError::not_found(format!("user {id} not found")));
        }
        audit_user(
            state,
            principal,
            principal.org_id(),
            "user.enable",
            &user,
            json!({}),
        )
        .await;
    }

    let updated = state
        .db
        .users
        .get(id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("user {id} not found")))?;
    Ok(Json(updated))
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
        acl::ensure_action(&principal, Action::ManageMembers)?;
        if !principal.is_legacy_admin()
            && !shares_managed_organization(&state, &principal, &user.id).await?
        {
            return Err(AppError::not_found(format!("user {id} not found")));
        }
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
        principal.org_id(),
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

/// Whether `user_id` belongs to an organization the caller administers.
///
/// Sharing any organization is not enough: the caller must hold a role that
/// grants `ManageMembers` there, otherwise a viewer in one organization could
/// manage accounts that happen to be members of it.
async fn shares_managed_organization(
    state: &AppState,
    principal: &Principal,
    user_id: &str,
) -> Result<bool, AppError> {
    for membership in principal.memberships() {
        if !Policy::allows(membership.role, Action::ManageMembers) {
            continue;
        }
        if state
            .db
            .memberships
            .get(&membership.org_id, user_id)
            .await?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn principal_user_id(principal: &Principal) -> Option<&str> {
    match &principal.kind {
        PrincipalKind::LegacyAdmin => None,
        PrincipalKind::User(user) => Some(user.user.id.as_str()),
    }
}

/// Append an identity-unlink audit record without failing the request.
async fn audit_identity_unlink(
    state: &AppState,
    principal: &Principal,
    user_id: &str,
    provider: &str,
) {
    if let Err(error) = state
        .db
        .audit
        .append(
            &principal.actor,
            None,
            "auth.identity.unlink",
            user_id,
            json!({"provider": provider}),
        )
        .await
    {
        tracing::warn!(error = %error, "failed to audit an identity unlink");
    }
}

async fn audit_user(
    state: &AppState,
    principal: &Principal,
    org_id: Option<&str>,
    action: &str,
    user: &User,
    details: serde_json::Value,
) {
    if let Err(error) = state
        .db
        .audit
        .append(&principal.actor, org_id, action, &user.id, details)
        .await
    {
        tracing::warn!(error = %error, action, "failed to audit user action");
    }
}

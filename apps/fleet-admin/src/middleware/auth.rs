//! Session and bearer-token authentication plus principal resolution.
//!
//! Two credentials are accepted:
//!
//! - A browser session cookie (`auth_sessions`), set by the OAuth callback.
//! - A per-user bearer token (`fla_...`), used by the CLI and automation.
//!
//! The legacy shared admin token is kept only as a bootstrap credential: it is
//! accepted for `/api/v1/users` and `/api/v1/orgs` so an operator can create
//! the first account. Every other route, for a deployment with at least one
//! user, requires a real identity.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fleet_core_auth::hash_token;
use fleet_core_common::error_codes;
use fleet_core_protocol::authz::{Membership, User, UserRole};

use crate::error::AppError;
use crate::state::AppState;

/// Header selecting the organization a request is scoped to.
pub const ORG_HEADER: &str = "x-fleet-org";

/// Authenticated caller.
#[derive(Debug, Clone)]
pub enum PrincipalKind {
    /// The legacy shared admin token. Bootstrap only; not scoped to an org.
    LegacyAdmin,
    /// A persisted user with its memberships.
    User(UserPrincipal),
}

/// A user and every organization it belongs to.
#[derive(Debug, Clone)]
pub struct UserPrincipal {
    /// The account.
    pub user: User,
    /// Memberships, oldest first. Never empty for a live account.
    pub memberships: Vec<Membership>,
}

/// Request identity plus the active organization.
#[derive(Debug, Clone)]
pub struct Principal {
    /// How the caller authenticated.
    pub kind: PrincipalKind,
    /// Active organization (`None` for the legacy bootstrap admin).
    pub org_id: Option<String>,
    /// Role in the active organization.
    pub role: UserRole,
    /// Name recorded in audit logs.
    pub actor: String,
}

impl Principal {
    /// The authenticated user, if any.
    pub fn user(&self) -> Option<&User> {
        match &self.kind {
            PrincipalKind::LegacyAdmin => None,
            PrincipalKind::User(principal) => Some(&principal.user),
        }
    }

    /// Memberships of the authenticated user.
    pub fn memberships(&self) -> &[Membership] {
        match &self.kind {
            PrincipalKind::LegacyAdmin => &[],
            PrincipalKind::User(principal) => &principal.memberships,
        }
    }

    /// Whether this is the legacy bootstrap admin token.
    pub fn is_legacy_admin(&self) -> bool {
        matches!(self.kind, PrincipalKind::LegacyAdmin)
    }

    /// Active organization scope, if the caller is a user.
    pub fn org_id(&self) -> Option<&str> {
        self.org_id.as_deref()
    }

    /// The caller's role in `org_id`, if they belong to it.
    pub fn role_in(&self, org_id: &str) -> Option<UserRole> {
        if self.is_legacy_admin() {
            return Some(UserRole::Owner);
        }
        self.memberships()
            .iter()
            .find(|membership| membership.org_id == org_id)
            .map(|membership| membership.role)
    }

    /// Whether the caller may write in the active organization.
    pub fn can_write(&self) -> bool {
        self.role.can_write()
    }

    /// Whether the caller may access resources owned by `org_id`.
    ///
    /// Membership, not the active organization, decides access. The legacy
    /// bootstrap admin sees every organization so it can recover unowned state.
    pub fn can_access_org(&self, org_id: Option<&str>) -> bool {
        if self.is_legacy_admin() {
            return true;
        }
        match org_id {
            Some(owner) => self.role_in(owner).is_some(),
            None => false,
        }
    }
}

/// Whether a principal was built from the legacy bootstrap token.
pub fn is_bootstrap_token(state: &AppState, credentials: &str) -> bool {
    state.auth.verify_raw(credentials).is_ok()
}

/// Require a valid session or bearer token and attach a [`Principal`].
///
/// `/api/v1/health`, `/api/v1/ready` and the browser event socket are mounted
/// outside this middleware (the socket authenticates from a ticket).
///
/// Viewers are restricted to safe methods here, with one narrow exception:
/// they may rotate their own token. The rotation handler still enforces
/// self-or-admin, and every other admin-only endpoint performs its own role
/// checks.
pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let principal = match resolve_principal(&state, request.headers()).await {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };

    // Cookie-authenticated writes need the double-submit CSRF token. Bearer
    // requests are exempt: a cross-site form cannot set that header.
    let used_bearer = has_bearer_credentials(request.headers());
    if !used_bearer
        && !is_safe_method(request.method())
        && !crate::auth::csrf_valid(request.headers())
    {
        return AppError::new(
            StatusCode::FORBIDDEN,
            error_codes::FORBIDDEN,
            "missing or invalid CSRF token",
        )
        .into_response();
    }

    let viewer_rotation =
        *request.method() == Method::POST && is_token_rotation_path(request.uri().path());
    let session_bootstrap =
        *request.method() == Method::POST && is_session_bootstrap_path(request.uri().path());
    if !principal.can_write()
        && !is_safe_method(request.method())
        && !viewer_rotation
        && !session_bootstrap
    {
        return AppError::new(
            StatusCode::FORBIDDEN,
            error_codes::FORBIDDEN,
            "this role has read-only access",
        )
        .into_response();
    }

    request.extensions_mut().insert(principal);
    next.run(request).await
}

/// Resolve a request to a [`Principal`].
///
/// The active organization comes from the `X-Fleet-Org` header. Without it the
/// user's personal organization is used, falling back to the first
/// membership. A header naming an organization the user does not belong to is
/// a `403`, never a silent fallback.
pub async fn resolve_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Principal, AppError> {
    let header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let requested_org = requested_org(headers);

    if let Some(credentials) = bearer_credentials(header) {
        return resolve_token(state, credentials, requested_org).await;
    }
    resolve_session(state, headers, requested_org).await
}

/// The organization named by the `X-Fleet-Org` header, if any.
pub fn requested_org(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(ORG_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Resolve an explicit bearer token. Used by the CLI and the event socket.
pub async fn resolve_token(
    state: &AppState,
    credentials: &str,
    requested_org: Option<&str>,
) -> Result<Principal, AppError> {
    if is_bootstrap_token(state, credentials) {
        return Ok(bootstrap_principal("admin".to_string()));
    }
    match state.db.users.by_token_hash(&hash_token(credentials)).await {
        Ok(Some(user)) => {
            // Token lifetimes apply to the bearer credential, not to a browser
            // session that was created while the token was still valid.
            if user
                .token_expires_at
                .is_some_and(|expires_at| expires_at <= fleet_core_common::now())
            {
                return Err(AppError::new(
                    StatusCode::UNAUTHORIZED,
                    error_codes::TOKEN_EXPIRED,
                    "token has expired",
                ));
            }
            build_principal(state, user, requested_org).await
        }
        Ok(None) => Err(AppError::unauthorized("missing or invalid bearer token")),
        Err(error) => {
            tracing::error!(error = %error, "failed to resolve bearer token");
            Err(AppError::internal("authentication lookup failed"))
        }
    }
}

fn bootstrap_principal(actor: String) -> Principal {
    Principal {
        kind: PrincipalKind::LegacyAdmin,
        org_id: None,
        role: UserRole::Owner,
        actor,
    }
}

async fn resolve_session(
    state: &AppState,
    headers: &HeaderMap,
    requested_org: Option<&str>,
) -> Result<Principal, AppError> {
    let Some(session_hash) = crate::auth::session_cookie_hash_from(headers) else {
        return Err(AppError::unauthorized(
            "missing or invalid bearer token or session cookie",
        ));
    };
    let session = state
        .db
        .auth_sessions
        .active(&session_hash)
        .await
        .map_err(|error| AppError::internal(error.to_string()))?;
    let Some(session) = session else {
        return Err(AppError::unauthorized("session is expired or revoked"));
    };
    let idle_days = i64::try_from(state.config.auth.session_idle_ttl_days).unwrap_or(i64::MAX / 2);
    if session.is_idle_for(chrono::Duration::days(idle_days)) {
        if let Err(error) = state.db.auth_sessions.revoke(&session_hash).await {
            tracing::debug!(error = %error, "failed to revoke an idle session");
        }
        return Err(AppError::unauthorized("session is expired or revoked"));
    }
    let user = state
        .db
        .users
        .get(&session.user_id)
        .await
        .map_err(|error| AppError::internal(error.to_string()))?
        .ok_or_else(|| AppError::unauthorized("session user no longer exists"))?;
    // Session activity is best-effort: a failure must not reject the request.
    if let Err(error) = state.db.auth_sessions.touch(&session_hash).await {
        tracing::debug!(error = %error, "failed to record session activity");
    }
    build_principal(state, user, requested_org).await
}

/// Build a principal for `user`, scoped to `requested_org` or the default.
///
/// Shared by the REST middleware, the event socket and session bootstrap so
/// membership, disabled-account and expiry checks exist exactly once.
pub async fn build_principal(
    state: &AppState,
    user: User,
    requested_org: Option<&str>,
) -> Result<Principal, AppError> {
    if user.disabled_at.is_some() {
        return Err(AppError::forbidden("this account has been disabled"));
    }

    let memberships = state
        .db
        .memberships
        .for_user(&user.id)
        .await
        .map_err(|error| AppError::internal(error.to_string()))?;
    if memberships.is_empty() {
        return Err(AppError::forbidden(
            "this account belongs to no organization",
        ));
    }

    let active = match requested_org {
        Some(org_id) => memberships
            .iter()
            .find(|membership| membership.org_id == org_id)
            .ok_or_else(|| {
                AppError::forbidden(format!("you are not a member of organization {org_id}"))
            })?,
        // Without an explicit selection, prefer a team organization. The
        // personal space is hidden in the UI whenever a team exists, and the
        // CLI should see the same default the dashboard shows.
        None => {
            let personal = state
                .db
                .organizations
                .personal_for_user(&user.id)
                .await
                .map_err(|error| AppError::internal(error.to_string()))?
                .map(|organization| organization.id);
            let mut team = None;
            for membership in &memberships {
                if Some(&membership.org_id) == personal.as_ref() {
                    continue;
                }
                team = Some(membership);
                break;
            }
            team.or_else(|| {
                memberships
                    .iter()
                    .find(|membership| Some(&membership.org_id) == personal.as_ref())
            })
            .unwrap_or(&memberships[0])
        }
    };

    // Read everything out of `active` and `user` before moving the owned
    // values into the principal.
    let active_org_id = active.org_id.clone();
    let active_role = active.role;
    let actor = user.name.clone();
    Ok(Principal {
        kind: PrincipalKind::User(UserPrincipal { user, memberships }),
        org_id: Some(active_org_id),
        role: active_role,
        actor,
    })
}

/// Whether the request carries a parseable bearer credential.
pub fn has_bearer_credentials(headers: &HeaderMap) -> bool {
    bearer_credentials(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    )
    .is_some()
}

fn bearer_credentials(header: Option<&str>) -> Option<&str> {
    let header = header?.trim();
    let (scheme, credentials) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let credentials = credentials.trim();
    (!credentials.is_empty()).then_some(credentials)
}

fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Match exactly `[/api/v1]/users/<id>/rotate-token`.
///
/// Axum strips the `/api/v1` nest prefix before the middleware runs, but unit
/// callers may pass the full path. This unlocks the read-only role gate only;
/// the handler checks that the caller rotates its own token (or is an
/// administrator).
pub fn is_token_rotation_path(path: &str) -> bool {
    let path = path.strip_prefix("/api/v1").unwrap_or(path);
    path.strip_prefix("/users/")
        .and_then(|rest| rest.strip_suffix("/rotate-token"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

/// Match exactly `[/api/v1]/auth/session`.
///
/// Viewers are read-only, but every role needs to be able to upgrade its
/// bearer token into a browser session.
pub fn is_session_bootstrap_path(path: &str) -> bool {
    path.strip_prefix("/api/v1").unwrap_or(path) == "/auth/session"
}

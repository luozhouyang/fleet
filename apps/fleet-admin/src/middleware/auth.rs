//! Bearer-token authentication and principal resolution for the REST API.

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fleet_core_auth::hash_token;
use fleet_core_common::error_codes;
use fleet_core_protocol::authz::{User, UserRole};

use crate::error::AppError;
use crate::state::AppState;

/// Authenticated caller resolved from a bearer token.
#[derive(Debug, Clone)]
pub enum PrincipalKind {
    /// The legacy shared admin token. Sees and manages every organization.
    LegacyAdmin,
    /// A persisted user with an organization and role.
    User(User),
}

/// Request identity plus authorization attributes.
#[derive(Debug, Clone)]
pub struct Principal {
    /// How the caller authenticated.
    pub kind: PrincipalKind,
    /// Organization scope (`None` for the legacy super-admin).
    pub org_id: Option<String>,
    /// Effective role.
    pub role: UserRole,
    /// Name recorded in audit logs.
    pub actor: String,
}

impl Principal {
    /// Whether this is the legacy shared admin token.
    pub fn is_legacy_admin(&self) -> bool {
        matches!(self.kind, PrincipalKind::LegacyAdmin)
    }

    /// Organization scope, if the caller is a user.
    pub fn org_id(&self) -> Option<&str> {
        self.org_id.as_deref()
    }

    /// Whether the caller may write.
    pub fn can_write(&self) -> bool {
        self.role.can_write()
    }

    /// Whether the caller may manage users and organizations.
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    /// Whether the caller may access resources owned by `org_id`.
    ///
    /// The legacy admin sees every organization; unowned resources are only
    /// visible to the legacy admin.
    pub fn can_access_org(&self, org_id: Option<&str>) -> bool {
        if self.is_legacy_admin() {
            return true;
        }
        match (self.org_id(), org_id) {
            (Some(scope), Some(owner)) => scope == owner,
            _ => false,
        }
    }
}

/// Require a valid bearer token and attach a [`Principal`] extension.
///
/// `/api/v1/health` is mounted outside this middleware, and the browser event
/// WebSocket performs its own header-or-query authentication because browser
/// WebSocket clients cannot set request headers.
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
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let Some(credentials) = bearer_credentials(header) else {
        return AppError::unauthorized("missing or invalid bearer token").into_response();
    };

    let principal = match resolve_principal(&state, credentials).await {
        Ok(principal) => principal,
        Err(error) => return error.into_response(),
    };

    let viewer_rotation =
        *request.method() == Method::POST && is_token_rotation_path(request.uri().path());
    if !principal.can_write() && !is_safe_method(request.method()) && !viewer_rotation {
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

/// Resolve a raw bearer token to a [`Principal`].
///
/// Used by the REST middleware and by the browser event WebSocket, which has
/// to authenticate from a query parameter.
pub async fn resolve_principal(state: &AppState, credentials: &str) -> Result<Principal, AppError> {
    if state.auth.verify_raw(credentials).is_ok() {
        return Ok(Principal {
            kind: PrincipalKind::LegacyAdmin,
            org_id: None,
            role: UserRole::Admin,
            actor: "admin".to_string(),
        });
    }
    match state.db.users.by_token_hash(&hash_token(credentials)).await {
        Ok(Some(user)) => {
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
            Ok(Principal {
                org_id: Some(user.org_id.clone()),
                role: user.role,
                actor: user.name.clone(),
                kind: PrincipalKind::User(user),
            })
        }
        Ok(None) => Err(AppError::unauthorized("missing or invalid bearer token")),
        Err(error) => {
            tracing::error!(error = %error, "failed to resolve bearer token");
            Err(AppError::internal("authentication lookup failed"))
        }
    }
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
fn is_token_rotation_path(path: &str) -> bool {
    let path = path.strip_prefix("/api/v1").unwrap_or(path);
    path.strip_prefix("/users/")
        .and_then(|rest| rest.strip_suffix("/rotate-token"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(role: UserRole, org: &str) -> Principal {
        Principal {
            kind: PrincipalKind::User(User {
                id: "user_1".into(),
                org_id: org.to_string(),
                name: "Ada".into(),
                role,
                created_at: fleet_core_common::now(),
                updated_at: fleet_core_common::now(),
                token_issued_at: Some(fleet_core_common::now()),
                token_expires_at: None,
            }),
            org_id: Some(org.to_string()),
            role,
            actor: "Ada".into(),
        }
    }

    #[test]
    fn bearer_parsing_is_case_insensitive_and_strict() {
        assert_eq!(bearer_credentials(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer_credentials(Some("  bearer   abc ")), Some("abc"));
        assert_eq!(bearer_credentials(Some("Basic abc")), None);
        assert_eq!(bearer_credentials(Some("Bearer ")), None);
        assert_eq!(bearer_credentials(None), None);
    }

    #[test]
    fn legacy_admin_sees_every_organization() {
        let principal = Principal {
            kind: PrincipalKind::LegacyAdmin,
            org_id: None,
            role: UserRole::Admin,
            actor: "admin".into(),
        };
        assert!(principal.can_access_org(Some("org_a")));
        assert!(principal.can_access_org(Some("org_b")));
        assert!(principal.is_admin());
        assert!(principal.can_write());
    }

    #[test]
    fn users_are_scoped_to_their_organization_and_role() {
        let member = user(UserRole::Member, "org_a");
        assert!(member.can_access_org(Some("org_a")));
        assert!(!member.can_access_org(Some("org_b")));
        assert!(!member.can_access_org(None));
        assert!(member.can_write());
        assert!(!member.is_admin());

        let viewer = user(UserRole::Viewer, "org_a");
        assert!(!viewer.can_write());
        assert!(viewer.can_access_org(Some("org_a")));
    }

    #[test]
    fn viewer_write_exception_is_limited_to_token_rotation() {
        assert!(is_token_rotation_path("/api/v1/users/user_1/rotate-token"));
        assert!(is_token_rotation_path("/users/user_1/rotate-token"));
        assert!(!is_token_rotation_path("/api/v1/users/user_1"));
        assert!(!is_token_rotation_path("/api/v1/users"));
        assert!(!is_token_rotation_path(
            "/api/v1/users/user_1/rotate-token/extra"
        ));
        assert!(!is_token_rotation_path("/api/v1/orgs"));
    }
}

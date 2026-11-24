//! Browser session bootstrap and logout.
//!
//! `POST /api/v1/auth/session` upgrades a user bearer token into an HttpOnly
//! session cookie plus a readable CSRF cookie. It exists so the dashboard does
//! not have to keep a long-lived credential in JavaScript; the OAuth callback
//! will create the same kind of session once provider login lands.
//!
//! `POST /api/v1/auth/logout` revokes the presented session. It is deliberately
//! unauthenticated so an expired session can still clear its cookies, but it
//! requires the CSRF token whenever a session cookie is present.

use axum::extract::State;
use axum::http::header::{self, HeaderMap};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use fleet_core_auth::hash_token;
use fleet_core_common::now;
use fleet_core_protocol::authz::{Me, PrincipalKind as AuthPrincipalKind};
use serde_json::json;

use crate::auth::{
    append_cookie, clear_csrf_cookie, clear_session_cookie, csrf_cookie, csrf_valid, random_token,
    session_cookie, session_cookie_hash_from,
};
use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::state::AppState;

/// Browser origins accepted for session-changing requests.
const DEV_ORIGINS: [&str; 2] = ["http://localhost:5173", "http://127.0.0.1:5173"];

/// `POST /api/v1/auth/session`
pub async fn create_session(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    ensure_origin(&state, &headers)?;
    let user = principal.user().cloned().ok_or_else(|| {
        AppError::forbidden("the bootstrap admin token cannot create a browser session")
    })?;

    let secure = request_is_https(&state, &headers);
    let token = random_token(32);
    let csrf = random_token(24);
    let ttl_days = i64::try_from(state.config.auth.session_ttl_days).unwrap_or(i64::MAX);
    let expires_at = now() + chrono::Duration::days(ttl_days);

    state
        .db
        .auth_sessions
        .create(
            &hash_token(&token),
            &user.id,
            expires_at,
            header_value(&headers, header::USER_AGENT),
            forwarded_ip(&headers),
        )
        .await?;

    if let Err(error) = state
        .db
        .audit
        .append(
            &principal.actor,
            principal.org_id(),
            "auth.session.create",
            &user.id,
            json!({"expires_at": expires_at.to_rfc3339()}),
        )
        .await
    {
        tracing::warn!(error = %error, "failed to audit browser session creation");
    }

    let mut response = Json(Me {
        kind: AuthPrincipalKind::User,
        user: Some(user),
        org_id: principal.org_id().map(str::to_owned),
        role: principal.role,
    })
    .into_response();
    append_cookie(
        &mut response,
        session_cookie(&token, secure, ttl_days.saturating_mul(86_400)),
    );
    append_cookie(&mut response, csrf_cookie(&csrf, secure));
    Ok(response)
}

/// `POST /api/v1/auth/logout`
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    ensure_origin(&state, &headers)?;
    let session_hash = session_cookie_hash_from(&headers);
    if session_hash.is_some() && !csrf_valid(&headers) {
        return Err(AppError::forbidden("missing or invalid CSRF token"));
    }
    if let Some(hash) = session_hash {
        state.db.auth_sessions.revoke(&hash).await?;
    }

    let secure = request_is_https(&state, &headers);
    let mut response = StatusCode::NO_CONTENT.into_response();
    append_cookie(&mut response, clear_session_cookie(secure));
    append_cookie(&mut response, clear_csrf_cookie(secure));
    Ok(response)
}

/// Reject a browser request from an origin the deployment does not serve.
///
/// Requests without an `Origin` header are non-browser clients and pass; the
/// CSRF check still applies to cookie-authenticated writes.
pub(crate) fn ensure_origin(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(());
    };
    let origin = origin.trim().trim_end_matches('/');
    if state
        .config
        .server
        .public_url
        .as_deref()
        .is_some_and(|public_url| public_url.trim().trim_end_matches('/') == origin)
    {
        return Ok(());
    }
    if DEV_ORIGINS.contains(&origin) {
        return Ok(());
    }
    if let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        if origin == format!("http://{host}") || origin == format!("https://{host}") {
            return Ok(());
        }
    }
    Err(AppError::forbidden("origin is not allowed"))
}

/// Whether the browser reached this admin over HTTPS.
pub(crate) fn request_is_https(state: &AppState, headers: &HeaderMap) -> bool {
    if state
        .config
        .server
        .public_url
        .as_deref()
        .is_some_and(|public_url| public_url.trim().starts_with("https://"))
    {
        return true;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn forwarded_ip(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
}

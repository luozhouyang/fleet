//! OAuth login and callback.
//!
//! `GET /api/v1/auth/login/{provider}` sets a single-use state and PKCE
//! verifier, then redirects to the provider. The callback consumes the state
//! exactly once, exchanges the code, resolves or creates the account, and
//! starts a browser session before redirecting to the SPA.
//!
//! Errors redirect to `/?auth_error=<code>` so the login page can explain them
//! without exposing provider responses or storage details.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::Duration;
use fleet_admin_storage::oauth_states::OAuthState;
use fleet_core_auth::{hash_token, pkce_challenge};
use fleet_core_common::now;
use fleet_core_protocol::authz::{AuthOptions, User};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::random_token;
use crate::error::AppError;
use crate::oauth::{OAuthProfile, OAuthProviderKind};
use crate::routes::session::{attach_session_cookies, issue_browser_session, request_is_https};
use crate::services::org_service::OrgService;
use crate::state::AppState;

/// Lifetime of a pending authorization request.
const OAUTH_STATE_TTL_SECS: i64 = 600;

/// `GET /api/v1/auth/providers` – login options for the SPA.
pub async fn providers(State(state): State<AppState>) -> Result<Json<AuthOptions>, AppError> {
    let bootstrap_required = !state.db.users.any_exists().await?;
    Ok(Json(AuthOptions {
        providers: state
            .oauth
            .kinds()
            .into_iter()
            .map(|kind| kind.as_str().to_string())
            .collect(),
        bootstrap_required,
        bootstrap_email_configured: !state.config.auth.bootstrap_admin_emails.is_empty(),
    }))
}

/// `GET /api/v1/auth/login/{provider}` – start the authorization-code flow.
pub async fn login(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let Some(kind) = OAuthProviderKind::parse(&provider) else {
        return Ok(auth_error("unknown_provider"));
    };
    let Some(client) = state.oauth.get(kind) else {
        return Ok(auth_error("provider_not_configured"));
    };

    let redirect_uri = format!(
        "{}/api/v1/auth/callback/{provider}",
        request_origin(&state, &headers)
    );
    let state_token = random_token(32);
    let verifier = random_token(48);
    let challenge = pkce_challenge(&verifier);
    let nonce = (kind == OAuthProviderKind::Google).then(|| random_token(24));

    state
        .db
        .oauth_states
        .create(
            &hash_token(&state_token),
            &OAuthState {
                provider: provider.clone(),
                pkce_verifier: verifier,
                nonce: nonce.clone(),
                redirect_uri: redirect_uri.clone(),
                expires_at: now() + Duration::seconds(OAUTH_STATE_TTL_SECS),
            },
        )
        .await?;
    let url = client.authorize_url(&redirect_uri, &state_token, &challenge, nonce.as_deref());
    Ok(Redirect::to(&url).into_response())
}

/// Query parameters the provider appends to the callback.
#[derive(Debug, Deserialize, Default)]
pub struct OAuthCallbackQuery {
    /// Authorization code.
    #[serde(default)]
    pub code: Option<String>,
    /// State echoed from the authorization request.
    #[serde(default)]
    pub state: Option<String>,
    /// Provider error code.
    #[serde(default)]
    pub error: Option<String>,
    /// Provider error description.
    #[serde(default)]
    pub error_description: Option<String>,
}

/// `GET /api/v1/auth/callback/{provider}` – finish the flow and start a session.
pub async fn callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
    headers: HeaderMap,
) -> Response {
    match finish_callback(&state, &provider, query, &headers).await {
        Ok(response) => response,
        Err(code) => {
            tracing::warn!(provider = %provider, outcome = code, "OAuth sign-in failed");
            auth_error(code)
        }
    }
}

async fn finish_callback(
    state: &AppState,
    provider: &str,
    query: OAuthCallbackQuery,
    headers: &HeaderMap,
) -> Result<Response, &'static str> {
    if let Some(error) = query.error.as_deref() {
        tracing::info!(
            provider,
            error,
            description = query.error_description.as_deref(),
            "OAuth provider denied the request"
        );
        return Err("provider_denied");
    }
    let kind = OAuthProviderKind::parse(provider).ok_or("unknown_provider")?;
    let code = query
        .code
        .filter(|value| !value.trim().is_empty())
        .ok_or("state_invalid")?;
    let state_token = query
        .state
        .filter(|value| !value.trim().is_empty())
        .ok_or("state_invalid")?;

    let record = state
        .db
        .oauth_states
        .consume(&hash_token(&state_token))
        .await
        .map_err(|error| internal(error, "oauth state lookup"))?
        .ok_or("state_invalid")?;
    if record.provider != provider {
        return Err("state_invalid");
    }
    let client = state.oauth.get(kind).ok_or("provider_not_configured")?;
    let profile = client
        .exchange(
            &code,
            &record.redirect_uri,
            &record.pkce_verifier,
            record.nonce.as_deref(),
        )
        .await
        .map_err(|error| {
            tracing::warn!(provider, error = %error, "OAuth code exchange failed");
            "exchange_failed"
        })?;
    if !profile.email_verified {
        return Err("email_unverified");
    }
    let Some(email) = verified_email(&profile) else {
        return Err("email_unverified");
    };

    let user = resolve_user(state, kind, &profile, &email).await?;
    if user.disabled_at.is_some() {
        return Err("account_disabled");
    }
    // Invitations created after the last sign-in are still pending.
    if let Err(error) = OrgService::accept_invitations_for_email(state, &user, &email).await {
        tracing::warn!(error = ?error, provider, "failed to accept pending invitations");
    }

    let issued = issue_browser_session(state, &user.id, headers)
        .await
        .map_err(|error| internal(error, "session creation"))?;
    let secure = request_is_https(state, headers);
    audit_login(state, &user, kind).await;

    let mut response = Redirect::to("/").into_response();
    attach_session_cookies(&mut response, &issued, secure);
    Ok(response)
}

/// Resolve the account for a verified profile, creating it when necessary.
async fn resolve_user(
    state: &AppState,
    kind: OAuthProviderKind,
    profile: &OAuthProfile,
    email: &str,
) -> Result<User, &'static str> {
    let provider = kind.as_str();

    if let Some(identity) = state
        .db
        .auth_identities
        .find(provider, &profile.subject)
        .await
        .map_err(|error| internal(error, "identity lookup"))?
    {
        let user = state
            .db
            .users
            .get(&identity.user_id)
            .await
            .map_err(|error| internal(error, "user lookup"))?
            .ok_or("internal")?;
        // Refresh the profile fields the provider owns; the account email is
        // left alone so a changed provider address cannot take over another
        // account through the unique email index.
        let name = display_name(profile, email);
        if let Err(error) = state
            .db
            .users
            .update_profile(&user.id, &name, profile.avatar_url.as_deref())
            .await
        {
            tracing::warn!(error = %error, "failed to refresh an OAuth profile");
        }
        let _ = state
            .db
            .auth_identities
            .touch(provider, &profile.subject)
            .await;
        return Ok(user);
    }

    if let Some(user) = state
        .db
        .users
        .by_email(email)
        .await
        .map_err(|error| internal(error, "email lookup"))?
    {
        state
            .db
            .auth_identities
            .link(
                &user.id,
                provider,
                &profile.subject,
                email,
                profile.login.as_deref(),
                profile.avatar_url.as_deref(),
            )
            .await
            .map_err(|error| internal(error, "identity link"))?;
        // Attaching another provider to an existing account is a security
        // boundary; the audit record shows both sides even when the sign-in
        // that follows is later revoked.
        audit_identity(
            state,
            &user,
            "auth.identity.link",
            json!({"provider": provider, "reason": "email_match"}),
        )
        .await;
        let _ = state
            .db
            .auth_identities
            .touch(provider, &profile.subject)
            .await;
        return Ok(user);
    }

    // The first account must come from the configured bootstrap list; once any
    // user exists the list is ignored and new accounts are self-service (they
    // start with a personal organization and no access to team resources).
    let first_account = !state
        .db
        .users
        .any_exists()
        .await
        .map_err(|error| internal(error, "bootstrap check"))?;
    if first_account {
        let allowed = state
            .config
            .auth
            .bootstrap_admin_emails
            .iter()
            .any(|allowed| allowed.trim().eq_ignore_ascii_case(email));
        if !allowed {
            return Err("not_initialized");
        }
    }

    let name = display_name(profile, email);
    let user = state
        .db
        .users
        .create_from_identity(email, &name, profile.avatar_url.as_deref())
        .await
        .map_err(|error| internal(error, "user creation"))?;
    OrgService::ensure_personal(state, &user.id, &user.name)
        .await
        .map_err(|error| internal(error, "personal organization"))?;
    state
        .db
        .auth_identities
        .link(
            &user.id,
            provider,
            &profile.subject,
            email,
            profile.login.as_deref(),
            profile.avatar_url.as_deref(),
        )
        .await
        .map_err(|error| internal(error, "identity link"))?;
    audit_identity(
        state,
        &user,
        "user.create",
        json!({
            "provider": provider,
            "source": "oauth",
            "bootstrap": first_account,
        }),
    )
    .await;
    Ok(user)
}

/// Normalize the provider email, requiring the `local@domain` shape.
fn verified_email(profile: &OAuthProfile) -> Option<String> {
    profile
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| email.contains('@'))
        .map(str::to_lowercase)
}

fn display_name(profile: &OAuthProfile, email: &str) -> String {
    let name = profile.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    email
        .split('@')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("User")
        .to_string()
}

/// External origin the callback must return to.
///
/// `server.public_url` wins because the callback URL must match the provider
/// registration exactly. Without it, the request host is used; a development
/// reverse proxy that rewrites `Host` therefore needs `public_url` set.
pub(crate) fn request_origin(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(public_url) = state.config.server.public_url.as_deref() {
        let trimmed = public_url.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1:8787");
    format!("{proto}://{host}")
}

/// Append an account or identity audit record without failing the sign-in.
async fn audit_identity(state: &AppState, user: &User, action: &str, details: Value) {
    if let Err(error) = state
        .db
        .audit
        .append(&user.name, None, action, &user.id, details)
        .await
    {
        tracing::warn!(error = %error, action, "failed to audit OAuth identity event");
    }
}

async fn audit_login(state: &AppState, user: &User, kind: OAuthProviderKind) {
    if let Err(error) = state
        .db
        .audit
        .append(
            &user.name,
            None,
            "auth.login",
            &user.id,
            json!({"provider": kind.as_str()}),
        )
        .await
    {
        tracing::warn!(error = %error, "failed to audit OAuth sign-in");
    }
}

fn auth_error(code: &'static str) -> Response {
    Redirect::to(&format!("/?auth_error={code}")).into_response()
}

fn internal<E: std::fmt::Debug>(error: E, context: &'static str) -> &'static str {
    tracing::warn!(error = ?error, context, "OAuth callback failed");
    "internal"
}

//! OAuth authorization-code clients for GitHub and Google.
//!
//! The flow lives in `routes::oauth`; this module owns the provider-specific
//! URLs, the PKCE exchange and the profile normalization. Providers are behind
//! a trait so the callback flow can be tested without network access.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fleet_core_config::{AuthConfig, OAuthProviderConfig};
use serde::Deserialize;

const USER_AGENT: &str = concat!("fleet-admin/", env!("CARGO_PKG_VERSION"));

const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER: &str = "https://api.github.com/user";
const GITHUB_EMAILS: &str = "https://api.github.com/user/emails";

const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// Supported OAuth providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthProviderKind {
    /// GitHub OAuth app.
    Github,
    /// Google OpenID Connect client.
    Google,
}

impl OAuthProviderKind {
    /// Parse the provider name used in paths and configuration.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "github" => Some(Self::Github),
            "google" => Some(Self::Google),
            _ => None,
        }
    }

    /// Stable provider name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Google => "google",
        }
    }
}

/// Normalized profile returned by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProfile {
    /// Immutable provider subject id.
    pub subject: String,
    /// Verified email, when the provider exposes one.
    pub email: Option<String>,
    /// Whether the provider marked the email verified.
    pub email_verified: bool,
    /// Display name; may be empty when the provider has none.
    pub name: String,
    /// Provider login or username.
    pub login: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
}

/// Provider exchange failure. Messages are safe to log.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// The provider could not be reached.
    #[error("provider request failed")]
    Request,
    /// The provider rejected the authorization code or returned an error.
    #[error("provider rejected the exchange: {0}")]
    Exchange(String),
    /// The provider response did not have the expected shape.
    #[error("provider returned a malformed response")]
    Malformed,
}

/// One configured provider.
#[async_trait]
pub trait OAuthProvider: Send + Sync {
    /// Which provider this client talks to.
    fn kind(&self) -> OAuthProviderKind;

    /// Build the browser authorization URL.
    fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        challenge: &str,
        nonce: Option<&str>,
    ) -> String;

    /// Exchange an authorization code for a verified profile.
    async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        nonce: Option<&str>,
    ) -> Result<OAuthProfile, OAuthError>;
}

/// Configured providers, cheap to clone and share through [`AppState`].
///
/// [`AppState`]: crate::state::AppState
#[derive(Clone, Default)]
pub struct OAuthRegistry {
    providers: Vec<Arc<dyn OAuthProvider>>,
}

impl OAuthRegistry {
    /// Build the registry from configuration. Providers without a complete
    /// client credential are treated as disabled.
    pub fn from_config(auth: &AuthConfig) -> Self {
        let mut registry = Self::default();
        if let Some(config) = auth.github.as_ref().filter(|config| config.is_configured()) {
            registry.providers.push(Arc::new(HttpOAuthProvider::new(
                OAuthProviderKind::Github,
                config,
            )));
        }
        if let Some(config) = auth.google.as_ref().filter(|config| config.is_configured()) {
            registry.providers.push(Arc::new(HttpOAuthProvider::new(
                OAuthProviderKind::Google,
                config,
            )));
        }
        registry
    }

    /// Add a provider. Used by tests to inject a fake.
    #[allow(dead_code)]
    pub fn with_provider(mut self, provider: Arc<dyn OAuthProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Look up a provider.
    pub fn get(&self, kind: OAuthProviderKind) -> Option<Arc<dyn OAuthProvider>> {
        self.providers
            .iter()
            .find(|provider| provider.kind() == kind)
            .cloned()
    }

    /// Names of the configured providers.
    pub fn kinds(&self) -> Vec<OAuthProviderKind> {
        self.providers
            .iter()
            .map(|provider| provider.kind())
            .collect()
    }
}

struct HttpOAuthProvider {
    kind: OAuthProviderKind,
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
}

impl HttpOAuthProvider {
    fn new(kind: OAuthProviderKind, config: &OAuthProviderConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            kind,
            client_id: config.client_id.trim().to_string(),
            client_secret: config.client_secret.trim().to_string(),
            http,
        }
    }

    async fn exchange_github(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<OAuthProfile, OAuthError> {
        let response = self
            .http
            .post(GITHUB_TOKEN)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|_| OAuthError::Request)?;
        if !response.status().is_success() {
            return Err(OAuthError::Exchange(format!(
                "token endpoint returned {}",
                response.status()
            )));
        }
        let token: GithubToken = response.json().await.map_err(|_| OAuthError::Malformed)?;
        let Some(access_token) = token.access_token else {
            return Err(OAuthError::Exchange(
                token
                    .error_description
                    .or(token.error)
                    .unwrap_or_else(|| "no access token".to_string()),
            ));
        };

        let user: GithubUser = self
            .http
            .get(GITHUB_USER)
            .bearer_auth(&access_token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(|_| OAuthError::Request)?
            .json()
            .await
            .map_err(|_| OAuthError::Malformed)?;
        let emails: Vec<GithubEmail> = self
            .http
            .get(GITHUB_EMAILS)
            .bearer_auth(&access_token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(|_| OAuthError::Request)?
            .json()
            .await
            .map_err(|_| OAuthError::Malformed)?;

        // Only a verified address may key an account. Prefer the primary one.
        let verified = emails
            .iter()
            .find(|email| email.primary && email.verified)
            .or_else(|| emails.iter().find(|email| email.verified));
        Ok(OAuthProfile {
            subject: user.id.to_string(),
            email: verified.map(|email| email.email.clone()),
            email_verified: verified.is_some(),
            name: user
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| user.login.clone()),
            login: Some(user.login),
            avatar_url: user.avatar_url,
        })
    }

    async fn exchange_google(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        nonce: Option<&str>,
    ) -> Result<OAuthProfile, OAuthError> {
        let response = self
            .http
            .post(GOOGLE_TOKEN)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|_| OAuthError::Request)?;
        if !response.status().is_success() {
            return Err(OAuthError::Exchange(format!(
                "token endpoint returned {}",
                response.status()
            )));
        }
        let token: GoogleToken = response.json().await.map_err(|_| OAuthError::Malformed)?;
        let Some(access_token) = token.access_token else {
            return Err(OAuthError::Exchange(
                token
                    .error_description
                    .or(token.error)
                    .unwrap_or_else(|| "no access token".to_string()),
            ));
        };

        // The token comes straight from the provider over TLS, so checking its
        // claims (rather than re-verifying the signature) is enough here. The
        // nonce binds this response to the authorization request.
        if let Some(expected) = nonce {
            let claims = token
                .id_token
                .as_deref()
                .and_then(decode_id_token_claims)
                .ok_or(OAuthError::Malformed)?;
            if claims.aud.as_deref() != Some(self.client_id.as_str())
                || claims.nonce.as_deref() != Some(expected)
            {
                return Err(OAuthError::Exchange("id token claims do not match".into()));
            }
        }

        let user: GoogleUser = self
            .http
            .get(GOOGLE_USERINFO)
            .bearer_auth(&access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(|_| OAuthError::Request)?
            .json()
            .await
            .map_err(|_| OAuthError::Malformed)?;
        Ok(OAuthProfile {
            subject: user.sub,
            email: user.email,
            email_verified: user.email_verified.unwrap_or(false),
            name: user.name.unwrap_or_default(),
            login: None,
            avatar_url: user.picture,
        })
    }
}

#[async_trait]
impl OAuthProvider for HttpOAuthProvider {
    fn kind(&self) -> OAuthProviderKind {
        self.kind
    }

    fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        challenge: &str,
        nonce: Option<&str>,
    ) -> String {
        let (endpoint, scope) = match self.kind {
            OAuthProviderKind::Github => (GITHUB_AUTHORIZE, "read:user user:email"),
            OAuthProviderKind::Google => (GOOGLE_AUTHORIZE, "openid email profile"),
        };
        let Ok(mut url) = reqwest::Url::parse(endpoint) else {
            return endpoint.to_string();
        };
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("client_id", &self.client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("response_type", "code")
                .append_pair("scope", scope)
                .append_pair("state", state)
                .append_pair("code_challenge", challenge)
                .append_pair("code_challenge_method", "S256");
            if let Some(nonce) = nonce {
                query.append_pair("nonce", nonce);
            }
        }
        url.to_string()
    }

    async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        nonce: Option<&str>,
    ) -> Result<OAuthProfile, OAuthError> {
        match self.kind {
            OAuthProviderKind::Github => self.exchange_github(code, redirect_uri, verifier).await,
            OAuthProviderKind::Google => {
                self.exchange_google(code, redirect_uri, verifier, nonce)
                    .await
            }
        }
    }
}

#[derive(Deserialize)]
struct GithubToken {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    id: u64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[derive(Deserialize)]
struct GoogleToken {
    access_token: Option<String>,
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct IdTokenClaims {
    aud: Option<String>,
    nonce: Option<String>,
}

#[derive(Deserialize)]
struct GoogleUser {
    sub: String,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    picture: Option<String>,
}

/// Decode the claims of an ID token received directly from the token endpoint.
///
/// Signature verification is intentionally skipped: OIDC allows relying on TLS
/// server validation for a token obtained this way. The claims we read (`aud`,
/// `nonce`) defend against a response for another client or request.
fn decode_id_token_claims(token: &str) -> Option<IdTokenClaims> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_round_trip() {
        for kind in [OAuthProviderKind::Github, OAuthProviderKind::Google] {
            assert_eq!(OAuthProviderKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(OAuthProviderKind::parse("gitlab"), None);
    }

    #[test]
    fn authorize_urls_carry_pkce_and_scope() {
        let provider = HttpOAuthProvider::new(
            OAuthProviderKind::Google,
            &OAuthProviderConfig {
                client_id: "client-1".into(),
                client_secret: "secret".into(),
            },
        );
        let url = provider.authorize_url(
            "https://fleet.example.com/api/v1/auth/callback/google",
            "state-1",
            "challenge-1",
            Some("nonce-1"),
        );
        assert!(url.starts_with(GOOGLE_AUTHORIZE));
        assert!(url.contains("code_challenge=challenge-1"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=openid+email+profile"));
        assert!(url.contains("nonce=nonce-1"));
        assert!(url.contains("state=state-1"));
    }

    #[test]
    fn id_token_claims_are_decoded() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"aud":"client-1","nonce":"nonce-1"}"#);
        let token = format!("header.{payload}.signature");
        let claims = decode_id_token_claims(&token).expect("claims");
        assert_eq!(claims.aud.as_deref(), Some("client-1"));
        assert_eq!(claims.nonce.as_deref(), Some("nonce-1"));
    }
}

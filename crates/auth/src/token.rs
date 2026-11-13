//! Shared-token authentication used for the control plane and daemon APIs.

use std::fmt;

use subtle::ConstantTimeEq;

use crate::error::{AuthError, Result};

/// Environment variable holding the control-plane admin token.
pub const ADMIN_TOKEN_ENV: &str = "AGENT_FLEET_ADMIN_TOKEN";
/// Environment variable holding the daemon/machine token.
pub const MACHINE_TOKEN_ENV: &str = "AGENT_FLEET_TOKEN";
/// Environment variable that opts into the development token when set to `1`.
pub const DEV_MODE_ENV: &str = "AGENT_FLEET_DEV";
/// Token used when `AGENT_FLEET_DEV=1` and no token is configured.
pub const DEV_TOKEN: &str = "dev-token";

/// Bearer-token authenticator with constant-time comparison.
#[derive(Clone)]
pub struct TokenAuth {
    token: String,
}

impl fmt::Debug for TokenAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenAuth")
            .field("token", &"<redacted>")
            .finish()
    }
}

impl TokenAuth {
    /// Create an authenticator for `token`.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Build an authenticator from the environment.
    ///
    /// Precedence:
    /// 1. `AGENT_FLEET_ADMIN_TOKEN` (control plane)
    /// 2. `AGENT_FLEET_TOKEN` (machine daemon)
    /// 3. `dev-token`, only when `AGENT_FLEET_DEV=1`
    ///
    /// Returns [`AuthError::TokenNotConfigured`] when none of those apply.
    pub fn from_env() -> Result<Self> {
        if let Some(token) = admin_token_env() {
            return Ok(Self::new(token));
        }
        if let Some(token) = machine_token_env() {
            return Ok(Self::new(token));
        }
        if std::env::var(DEV_MODE_ENV)
            .map(|value| value.trim() == "1")
            .unwrap_or(false)
        {
            return Ok(Self::new(DEV_TOKEN));
        }
        Err(AuthError::TokenNotConfigured)
    }

    /// Verify an `Authorization` header value.
    ///
    /// The header must have the shape `Bearer <token>`; the scheme is matched
    /// case-insensitively and surrounding whitespace is ignored. Any other
    /// shape yields [`AuthError::Unauthorized`].
    pub fn verify_bearer(&self, header: Option<&str>) -> Result<()> {
        let header = header.ok_or(AuthError::Unauthorized)?.trim();
        let (scheme, credentials) = header.split_once(' ').ok_or(AuthError::Unauthorized)?;
        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(AuthError::Unauthorized);
        }
        let credentials = credentials.trim();
        if credentials.is_empty() {
            return Err(AuthError::Unauthorized);
        }
        self.verify_raw(credentials)
    }

    /// Verify a raw token using constant-time comparison.
    pub fn verify_raw(&self, token: &str) -> Result<()> {
        if self.token.is_empty() || token.is_empty() {
            return Err(AuthError::Unauthorized);
        }
        let matches = self.token.as_bytes().ct_eq(token.as_bytes());
        if bool::from(matches) {
            Ok(())
        } else {
            Err(AuthError::Unauthorized)
        }
    }
}

/// Read the control-plane admin token from `AGENT_FLEET_ADMIN_TOKEN`.
///
/// Empty or whitespace-only values are treated as unset.
pub fn admin_token_env() -> Option<String> {
    read_token_env(ADMIN_TOKEN_ENV)
}

/// Read the machine/daemon token from `AGENT_FLEET_TOKEN`.
///
/// Empty or whitespace-only values are treated as unset.
pub fn machine_token_env() -> Option<String> {
    read_token_env(MACHINE_TOKEN_ENV)
}

fn read_token_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Environment variables are process-global; serialize env-mutating tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn clear() -> Self {
            let names = [ADMIN_TOKEN_ENV, MACHINE_TOKEN_ENV, DEV_MODE_ENV];
            let saved = names
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();
            for name in names {
                std::env::remove_var(name);
            }
            Self { saved }
        }

        fn set(&self, name: &str, value: &str) {
            std::env::set_var(name, value);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn verify_raw_accepts_only_exact_token() {
        let auth = TokenAuth::new("s3cret");
        auth.verify_raw("s3cret").expect("correct token");
        assert!(matches!(
            auth.verify_raw("s3cre"),
            Err(AuthError::Unauthorized)
        ));
        assert!(matches!(
            auth.verify_raw("s3cret "),
            Err(AuthError::Unauthorized)
        ));
        assert!(matches!(auth.verify_raw(""), Err(AuthError::Unauthorized)));
    }

    #[test]
    fn verify_bearer_accepts_well_formed_headers() {
        let auth = TokenAuth::new("s3cret");
        auth.verify_bearer(Some("Bearer s3cret")).expect("plain");
        auth.verify_bearer(Some("bearer s3cret"))
            .expect("lowercase");
        auth.verify_bearer(Some("BEARER s3cret"))
            .expect("uppercase");
        auth.verify_bearer(Some("  Bearer   s3cret  "))
            .expect("trimmed, extra spaces");
    }

    #[test]
    fn verify_bearer_rejects_missing_wrong_or_malformed() {
        let auth = TokenAuth::new("s3cret");
        for header in [
            None,
            Some(""),
            Some("s3cret"),
            Some("Basic s3cret"),
            Some("Bearer"),
            Some("Bearer wrong"),
            Some("Bearer "),
            Some("Bearer\ts3cret"),
        ] {
            assert!(
                matches!(auth.verify_bearer(header), Err(AuthError::Unauthorized)),
                "header {header:?} should be rejected"
            );
        }
    }

    #[test]
    fn empty_configured_token_never_authenticates() {
        let auth = TokenAuth::new("");
        assert!(matches!(auth.verify_raw(""), Err(AuthError::Unauthorized)));
        assert!(matches!(
            auth.verify_bearer(Some("Bearer ")),
            Err(AuthError::Unauthorized)
        ));
    }

    #[test]
    fn from_env_prefers_admin_then_machine_then_dev() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        let env = EnvGuard::clear();
        env.set(DEV_MODE_ENV, "1");
        TokenAuth::from_env()
            .expect("dev fallback")
            .verify_raw(DEV_TOKEN)
            .expect("dev token");

        env.set(MACHINE_TOKEN_ENV, "machine-token");
        let auth = TokenAuth::from_env().expect("machine token");
        auth.verify_raw("machine-token").expect("machine token");
        assert!(auth.verify_raw(DEV_TOKEN).is_err());

        env.set(ADMIN_TOKEN_ENV, "admin-token");
        let auth = TokenAuth::from_env().expect("admin token");
        auth.verify_raw("admin-token").expect("admin token");
        assert!(auth.verify_raw("machine-token").is_err());
    }

    #[test]
    fn from_env_errors_when_nothing_configured() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        let env = EnvGuard::clear();
        assert!(matches!(
            TokenAuth::from_env(),
            Err(AuthError::TokenNotConfigured)
        ));

        env.set(DEV_MODE_ENV, "0");
        assert!(matches!(
            TokenAuth::from_env(),
            Err(AuthError::TokenNotConfigured)
        ));

        env.set(MACHINE_TOKEN_ENV, "   ");
        assert!(matches!(
            TokenAuth::from_env(),
            Err(AuthError::TokenNotConfigured)
        ));
    }

    #[test]
    fn env_helpers_ignore_empty_values() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        let env = EnvGuard::clear();
        assert_eq!(admin_token_env(), None);
        assert_eq!(machine_token_env(), None);

        env.set(ADMIN_TOKEN_ENV, "  admin  ");
        env.set(MACHINE_TOKEN_ENV, "machine");
        assert_eq!(admin_token_env().as_deref(), Some("admin"));
        assert_eq!(machine_token_env().as_deref(), Some("machine"));
    }

    #[test]
    fn debug_does_not_expose_token() {
        let auth = TokenAuth::new("super-secret-token");
        let rendered = format!("{auth:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("super-secret-token"));
    }
}

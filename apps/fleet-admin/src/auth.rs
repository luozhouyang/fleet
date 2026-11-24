//! Browser session cookie handling.
//!
//! The cookie carries an opaque session token. Only its SHA-256 hash is stored
//! in `auth_sessions`, so a leaked database does not leak live sessions.
//!
//! Over HTTPS the cookie is named `__Host-fleet_session`, which browsers only
//! accept with `Secure`, `Path=/` and no `Domain`. Plain-HTTP development on
//! localhost cannot use the `__Host-` prefix, so it falls back to a regular name.

// The browser session bootstrap and logout routes use the cookie and CSRF
// helpers below. The OAuth login routes are still missing, which is why not
// every helper has a production caller yet.
#![allow(dead_code)]

use axum::extract::Request;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::HeaderMap;
use axum::response::Response;
use fleet_core_auth::hash_token;

/// Cookie name used over HTTPS.
pub const SESSION_COOKIE: &str = "__Host-fleet_session";
/// Cookie name used for plain-HTTP development.
pub const DEV_SESSION_COOKIE: &str = "fleet_session";
/// Double-submit CSRF cookie.
pub const CSRF_COOKIE: &str = "fleet_csrf";
/// Header carrying the CSRF token on unsafe methods.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Name of the session cookie for the current scheme.
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        SESSION_COOKIE
    } else {
        DEV_SESSION_COOKIE
    }
}

/// Hash of the session token presented in the request, if any.
pub fn session_cookie_hash(request: &Request) -> Option<String> {
    session_cookie_hash_from(request.headers())
}

/// Hash of the session token presented in request headers, if any.
pub fn session_cookie_hash_from(headers: &HeaderMap) -> Option<String> {
    let raw = cookie_value(headers, SESSION_COOKIE)
        .or_else(|| cookie_value(headers, DEV_SESSION_COOKIE))?;
    let raw = raw.trim();
    (!raw.is_empty()).then(|| hash_token(raw))
}

/// Read one cookie value from request headers.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies = headers.get_all(COOKIE);
    for value in cookies.iter() {
        let Ok(text) = value.to_str() else { continue };
        for pair in text.split(';') {
            let pair = pair.trim();
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if key.trim() == name {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Build the `Set-Cookie` header that starts a session.
pub fn session_cookie(token: &str, secure: bool, max_age_secs: i64) -> String {
    let name = session_cookie_name(secure);
    let mut cookie = format!(
        "{name}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}",
        max_age = max_age_secs.max(0)
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Build the `Set-Cookie` header that clears the session.
pub fn clear_session_cookie(secure: bool) -> String {
    let name = session_cookie_name(secure);
    format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Build the CSRF cookie and the header value a client must echo back.
pub fn csrf_cookie(token: &str, secure: bool) -> String {
    let mut cookie = format!("{CSRF_COOKIE}={token}; Path=/; SameSite=Lax");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Build the `Set-Cookie` header that clears the CSRF cookie.
pub fn clear_csrf_cookie(secure: bool) -> String {
    let mut cookie = format!("{CSRF_COOKIE}=; Path=/; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Append a `Set-Cookie` header to a response.
pub fn append_cookie(response: &mut Response, cookie: String) {
    if let Ok(value) = cookie.parse() {
        response.headers_mut().append(SET_COOKIE, value);
    }
}

/// Whether the request presented the CSRF token it was issued.
///
/// The client sends the value both as a cookie and as `X-CSRF-Token`; a
/// mismatch means a cross-site form post, because another origin can send
/// cookies but cannot read them to fill the header.
pub fn csrf_valid(headers: &HeaderMap) -> bool {
    let Some(cookie) = cookie_value(headers, CSRF_COOKIE) else {
        return false;
    };
    let Some(header) = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    constant_time_eq(cookie.as_bytes(), header.trim().as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Generate an opaque random token, base64url without padding.
pub fn random_token(bytes: usize) -> String {
    use base64::Engine;
    let mut buffer = vec![0u8; bytes];
    // `OsRng` is used through `rand` so this never silently falls back to a
    // deterministic stream.
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut buffer);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    fn request_with_cookies(pairs: &[(&str, &str)]) -> Request {
        let header = pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        HttpRequest::builder()
            .header(COOKIE, header)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn cookie_names_follow_the_scheme() {
        assert_eq!(session_cookie_name(true), SESSION_COOKIE);
        assert_eq!(session_cookie_name(false), DEV_SESSION_COOKIE);
    }

    #[test]
    fn session_hash_reads_both_cookie_names() {
        let secure = request_with_cookies(&[(SESSION_COOKIE, "token")]);
        assert!(session_cookie_hash(&secure).is_some());
        let dev = request_with_cookies(&[(DEV_SESSION_COOKIE, "token")]);
        assert!(session_cookie_hash(&dev).is_some());
        let missing = request_with_cookies(&[("other", "value")]);
        assert!(session_cookie_hash(&missing).is_none());
    }

    #[test]
    fn secure_cookie_is_hardened() {
        let cookie = session_cookie("abc", true, 3600);
        assert!(cookie.starts_with("__Host-fleet_session=abc"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Secure"));
        assert!(!session_cookie("abc", false, 3600).contains("Secure"));
    }

    #[test]
    fn csrf_requires_matching_cookie_and_header() {
        let mut request = request_with_cookies(&[(CSRF_COOKIE, "csrf-token")]);
        assert!(!csrf_valid(request.headers()));
        request
            .headers_mut()
            .insert(CSRF_HEADER, "csrf-token".parse().unwrap());
        assert!(csrf_valid(request.headers()));
        request
            .headers_mut()
            .insert(CSRF_HEADER, "different".parse().unwrap());
        assert!(!csrf_valid(request.headers()));
    }

    #[test]
    fn random_tokens_are_unique_and_url_safe() {
        let first = random_token(32);
        let second = random_token(32);
        assert_ne!(first, second);
        assert!(first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}

//! `Idempotency-Key` parsing for retry-safe mutations.

use axum::http::HeaderMap;

use crate::error::AppError;

/// Standard header carrying a client-generated key for a retry-safe mutation.
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";

/// Longest accepted key.
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 200;

/// Parse and validate an optional `Idempotency-Key` header.
///
/// The key is forwarded to the daemon, which replays the cached response for a
/// duplicate instead of executing the command again.
pub fn idempotency_key(headers: &HeaderMap) -> Result<Option<String>, AppError> {
    let Some(value) = headers.get(IDEMPOTENCY_HEADER) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| AppError::bad_request("Idempotency-Key must be visible ASCII"))?
        .trim();
    if value.is_empty()
        || value.len() > MAX_IDEMPOTENCY_KEY_LEN
        || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(AppError::bad_request(
            "Idempotency-Key must be 1 to 200 printable ASCII characters",
        ));
    }
    Ok(Some(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(IDEMPOTENCY_HEADER, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn a_missing_header_has_no_key() {
        assert_eq!(idempotency_key(&HeaderMap::new()).unwrap(), None);
    }

    #[test]
    fn a_valid_key_is_trimmed() {
        assert_eq!(
            idempotency_key(&headers("  retry-1  ")).unwrap().as_deref(),
            Some("retry-1")
        );
    }

    #[test]
    fn empty_oversized_or_control_character_keys_are_rejected() {
        assert!(idempotency_key(&headers("   ")).is_err());
        let oversized = "k".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1);
        let mut map = HeaderMap::new();
        map.insert(
            IDEMPOTENCY_HEADER,
            HeaderValue::from_str(&oversized).unwrap(),
        );
        assert!(idempotency_key(&map).is_err());

        let mut map = HeaderMap::new();
        map.insert(
            IDEMPOTENCY_HEADER,
            HeaderValue::from_bytes(b"caf\xc3\xa9").unwrap(),
        );
        assert!(idempotency_key(&map).is_err());
    }
}

//! PKCE (RFC 7636) helpers for the OAuth authorization-code flow.

use base64::Engine;
use sha2::{Digest, Sha256};

/// S256 code challenge for a PKCE verifier.
///
/// The verifier stays server-side in `auth_oauth_states`; only this digest is
/// sent to the provider.
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_s256_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}

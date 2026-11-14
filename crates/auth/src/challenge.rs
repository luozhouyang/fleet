//! Challenge/nonce generation for replay-protected machine authentication.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rand::rngs::OsRng;
use rand::RngCore;

use agent_fleet_common::new_id;

/// Number of random bytes in a challenge nonce.
pub const NONCE_LEN: usize = 32;

/// A one-shot authentication challenge.
///
/// The daemon signs [`Self::nonce`] (or the base64 form returned by
/// [`Self::nonce_base64`]) with its machine identity; the agent admin tracks
/// [`Self::id`] to prevent replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// Unique challenge identifier, prefixed with `ch_`.
    pub id: String,
    /// Random nonce bytes.
    pub nonce: Vec<u8>,
}

impl Challenge {
    /// Generate a challenge with a fresh 32-byte nonce from the OS CSPRNG.
    pub fn new() -> Self {
        let mut nonce = vec![0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        Self {
            id: new_id("ch"),
            nonce,
        }
    }

    /// Standard base64 encoding of the nonce.
    pub fn nonce_base64(&self) -> String {
        STANDARD.encode(&self.nonce)
    }
}

impl Default for Challenge {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a fresh challenge. Convenience wrapper around [`Challenge::new`].
pub fn new_challenge() -> Challenge {
    Challenge::new()
}

/// Standard base64 encoding of a challenge nonce.
pub fn nonce_base64(challenge: &Challenge) -> String {
    challenge.nonce_base64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn challenge_has_32_byte_nonce_and_prefixed_id() {
        let challenge = new_challenge();
        assert_eq!(challenge.nonce.len(), NONCE_LEN);
        assert!(challenge.id.starts_with("ch_"));
        assert!(challenge.id.len() > "ch_".len());

        let decoded = STANDARD
            .decode(nonce_base64(&challenge))
            .expect("nonce is base64");
        assert_eq!(decoded, challenge.nonce);
        assert_eq!(challenge.nonce_base64(), nonce_base64(&challenge));
    }

    #[test]
    fn challenges_are_unique() {
        let mut ids = HashSet::new();
        let mut nonces = HashSet::new();
        for _ in 0..128 {
            let challenge = Challenge::new();
            assert!(ids.insert(challenge.id.clone()), "duplicate challenge id");
            assert!(nonces.insert(challenge.nonce.clone()), "duplicate nonce");
        }
    }

    #[test]
    fn nonce_is_not_all_zeros() {
        let challenge = Challenge::new();
        assert!(challenge.nonce.iter().any(|byte| *byte != 0));
    }
}

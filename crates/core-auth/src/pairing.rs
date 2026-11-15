//! Pairing-code and per-machine-token generation.

use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Number of symbols in a normalized pairing code.
pub const PAIR_CODE_LEN: usize = 12;

/// Crockford Base32: no I, L, O or U.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Generate a display code such as `7Q2M-9XKR-4TWD`.
pub fn generate_pair_code() -> String {
    let mut bytes = [0u8; PAIR_CODE_LEN];
    OsRng.fill_bytes(&mut bytes);
    let raw: String = bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12])
}

/// Normalize user input: uppercase, strip separators, map I/L to 1 and O to 0.
pub fn normalize_pair_code(input: &str) -> Option<String> {
    let mut normalized = String::with_capacity(PAIR_CODE_LEN);
    for ch in input.chars() {
        // Strip every Unicode whitespace character (space, tab, newline,
        // carriage return, ...) plus the display dash.
        if ch.is_whitespace() || ch == '-' {
            continue;
        }
        let ch = ch.to_ascii_uppercase();
        let mapped = match ch {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        };
        // Reject non-ASCII before the byte cast: `mapped as u8` truncates, so
        // e.g. U+0130 ('İ') would otherwise be accepted as '0'.
        if !mapped.is_ascii() || !ALPHABET.contains(&(mapped as u8)) {
            return None;
        }
        normalized.push(mapped);
    }
    (normalized.len() == PAIR_CODE_LEN).then_some(normalized)
}

/// SHA-256 of the normalized code, hex encoded.
pub fn hash_pair_code(input: &str) -> Option<String> {
    let normalized = normalize_pair_code(input)?;
    Some(hex::encode(Sha256::digest(normalized.as_bytes())))
}

/// Generate an `afm_<64 hex>` per-machine token.
pub fn generate_machine_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("afm_{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_have_the_expected_shape() {
        let code = generate_pair_code();
        assert_eq!(code.len(), 14);
        assert_eq!(code.chars().filter(|c| *c == '-').count(), 2);
        assert!(normalize_pair_code(&code).is_some());
    }

    #[test]
    fn normalization_accepts_case_separators_and_confusables() {
        assert_eq!(
            normalize_pair_code("abcd-efgh-jkmn").as_deref(),
            Some("ABCDEFGHJKMN")
        );
        assert_eq!(
            normalize_pair_code("il0o 7q2m9xkr").as_deref(),
            Some("11007Q2M9XKR")
        );
        assert!(normalize_pair_code("7Q2M-9XKR-4TWU").is_none());
        assert!(normalize_pair_code("short").is_none());
    }

    #[test]
    fn generated_codes_use_three_groups_from_the_alphabet() {
        let code = generate_pair_code();
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 3);
        for group in &groups {
            assert_eq!(group.chars().count(), 4);
            for symbol in group.chars() {
                assert!(symbol.is_ascii());
                assert!(ALPHABET.contains(&(symbol as u8)));
            }
        }
        for excluded in ['I', 'L', 'O', 'U'] {
            assert!(!code.contains(excluded));
        }
    }

    #[test]
    fn normalization_strips_every_whitespace_separator() {
        assert_eq!(
            normalize_pair_code("ABCD\nEFGH\tJKMN").as_deref(),
            Some("ABCDEFGHJKMN")
        );
        assert_eq!(
            normalize_pair_code("ABCD\r\nEFGH JKMN").as_deref(),
            Some("ABCDEFGHJKMN")
        );
        assert_eq!(
            normalize_pair_code("ABCD-EFGH-JKMN").as_deref(),
            Some("ABCDEFGHJKMN")
        );
    }

    #[test]
    fn normalization_rejects_non_ascii_symbols() {
        // Each input has 12 symbols but one of them is non-ASCII.
        assert_eq!(normalize_pair_code("İBCD-EFGH-JKMN"), None);
        assert_eq!(normalize_pair_code("０BCD-EFGH-JKMN"), None);
        assert_eq!(normalize_pair_code("éBCD-EFGH-JKMN"), None);
        assert_eq!(normalize_pair_code("ABCD-EFGH-JKMİ"), None);
    }

    #[test]
    fn normalization_still_maps_confusables() {
        assert_eq!(
            normalize_pair_code("ILO0-ABCD-EFGH").as_deref(),
            Some("1100ABCDEFGH")
        );
    }

    #[test]
    fn hashes_are_stable_and_normalized() {
        let a = hash_pair_code("abcd-efgh-jkmn").unwrap();
        let b = hash_pair_code("ABCDEFGHJKMN").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn machine_tokens_are_prefixed_and_unique() {
        let a = generate_machine_token();
        let b = generate_machine_token();
        assert!(a.starts_with("afm_"));
        assert_eq!(a.len(), 4 + 64);
        assert_ne!(a, b);
    }
}

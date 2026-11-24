//! In-memory plaintext for machine token rotations awaiting confirmation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Plaintext of rotations whose delivery or acknowledgement is unconfirmed.
///
/// Only the token hash is persisted. The plaintext stays in process memory so
/// an operator can retry a failed delivery with the *same* token: replacing a
/// pending token with a fresh one could lock out a daemon that had already
/// saved the first token but whose acknowledgement was lost.
#[derive(Clone, Default)]
pub struct TokenRotations {
    tokens: Arc<Mutex<HashMap<String, String>>>,
}

impl TokenRotations {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        // A poisoned lock only means another thread panicked while holding it;
        // the map itself is still usable.
        self.tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Plaintext token offered to `machine_id`, if a retry is still possible.
    pub fn get(&self, machine_id: &str) -> Option<String> {
        self.lock().get(machine_id).cloned()
    }

    /// Remember the token offered to `machine_id`, returning the previous one.
    pub fn insert(&self, machine_id: &str, token: String) -> Option<String> {
        self.lock().insert(machine_id.to_string(), token)
    }

    /// Forget the rotation for `machine_id` after confirmation or cancel.
    pub fn remove(&self, machine_id: &str) -> Option<String> {
        self.lock().remove(machine_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_forgets_the_plaintext() {
        let rotations = TokenRotations::default();
        assert_eq!(rotations.get("m1"), None);
        assert_eq!(rotations.insert("m1", "flm_a".into()), None);
        assert_eq!(rotations.get("m1").as_deref(), Some("flm_a"));
        assert_eq!(
            rotations.insert("m1", "flm_b".into()).as_deref(),
            Some("flm_a")
        );
        assert_eq!(rotations.get("m1").as_deref(), Some("flm_b"));
        assert_eq!(rotations.remove("m1").as_deref(), Some("flm_b"));
        assert_eq!(rotations.get("m1"), None);
    }
}

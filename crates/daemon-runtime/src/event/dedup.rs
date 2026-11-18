//! Bounded deduplication of native event entity ids.
//!
//! Providers may replay native notifications after a reconnect, deliver
//! overlapping session updates or emit the same tool-call update twice. The
//! runtime applies every event to Fleet state exactly once, so it needs a
//! cheap way to recognise an entity it has already observed. Keys are
//! `(kind, id)` pairs, where `kind` is a caller-supplied tag such as
//! `"message"` or `"tool"`.

use std::collections::{HashMap, VecDeque};

use parking_lot::Mutex;

/// A bounded, O(1) amortized deduplicator for event entity ids.
///
/// `EventDeduplicator` remembers the most recent `capacity` keys in insertion
/// order. When an insertion pushes the live key count over that bound, the
/// oldest half of the keys is evicted, so memory stays bounded even for a
/// long-running session while recent duplicates are still detected.
///
/// The type is safe to share between tasks: all mutation happens under a
/// [`parking_lot::Mutex`].
#[derive(Debug)]
pub struct EventDeduplicator {
    /// Maximum number of live keys retained.
    capacity: usize,
    /// Live keys and their insertion order.
    state: Mutex<DedupState>,
}

/// State guarded by the deduplicator mutex.
#[derive(Debug, Default)]
struct DedupState {
    /// Live keys mapped to the sequence number of their current insertion.
    entries: HashMap<(String, String), u64>,
    /// Insertion order. Entries whose sequence no longer matches `entries`
    /// belong to a forgotten insertion and are skipped lazily.
    order: VecDeque<(u64, (String, String))>,
    /// Monotonic sequence used to distinguish re-inserted keys.
    next_seq: u64,
}

impl EventDeduplicator {
    /// Create a deduplicator that retains at most `capacity` keys.
    ///
    /// A capacity of zero disables deduplication: [`Self::first_seen`] always
    /// returns `true` and nothing is retained.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(DedupState::default()),
        }
    }

    /// Record `(kind, id)` and report whether it is new.
    ///
    /// Returns `true` the first time a key is observed and `false` for every
    /// later observation. When the live key count exceeds `capacity`, the
    /// oldest half of the keys is evicted in insertion order.
    pub fn first_seen(&self, kind: &str, id: &str) -> bool {
        if self.capacity == 0 {
            return true;
        }
        let key = (kind.to_string(), id.to_string());
        let mut state = self.state.lock();
        if state.entries.contains_key(&key) {
            return false;
        }
        let seq = state.next_seq;
        state.next_seq += 1;
        state.entries.insert(key.clone(), seq);
        state.order.push_back((seq, key));
        if state.entries.len() > self.capacity {
            state.prune();
        }
        true
    }

    /// Forget a key so that a later [`Self::first_seen`] observes it again.
    ///
    /// This is used when an entity id is legitimately reused, for example
    /// when a provider restarts a tool call under the same id.
    pub fn forget(&self, kind: &str, id: &str) {
        let key = (kind.to_string(), id.to_string());
        let mut state = self.state.lock();
        state.entries.remove(&key);
        state.compact_if_needed(self.capacity);
    }

    /// Number of live keys currently retained.
    pub fn len(&self) -> usize {
        self.state.lock().entries.len()
    }

    /// Returns `true` when no keys are retained.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl DedupState {
    /// Evict the oldest half of the live keys.
    ///
    /// Stale order entries (left behind by [`EventDeduplicator::forget`]) are
    /// discarded as they surface and do not count towards the number of keys
    /// removed.
    fn prune(&mut self) {
        let mut remaining = self.entries.len() / 2;
        while remaining > 0 {
            let Some((seq, key)) = self.order.pop_front() else {
                break;
            };
            if self.entries.get(&key) == Some(&seq) {
                self.entries.remove(&key);
                remaining -= 1;
            }
        }
    }

    /// Drop stale order entries when they dominate the queue.
    ///
    /// Repeatedly forgetting and re-inserting a key leaves a stale order
    /// entry behind. Compaction is triggered only after the queue grows past
    /// twice the capacity, which keeps the amortized cost per operation
    /// constant while bounding the queue.
    fn compact_if_needed(&mut self, capacity: usize) {
        let limit = capacity.saturating_mul(2).max(8);
        if self.order.len() > limit {
            let entries = &self.entries;
            self.order
                .retain(|(seq, key)| entries.get(key) == Some(seq));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_message_id_is_seen_once() {
        let dedup = EventDeduplicator::new(16);
        assert!(dedup.first_seen("message", "msg_1"));
        assert!(!dedup.first_seen("message", "msg_1"));
        assert_eq!(dedup.len(), 1);
        assert!(!dedup.is_empty());
    }

    #[test]
    fn distinct_kinds_with_the_same_id_are_independent() {
        let dedup = EventDeduplicator::new(16);
        assert!(dedup.first_seen("message", "shared"));
        assert!(dedup.first_seen("tool", "shared"));
        assert!(!dedup.first_seen("message", "shared"));
        assert!(!dedup.first_seen("tool", "shared"));
        assert_eq!(dedup.len(), 2);
    }

    #[test]
    fn capacity_bound_is_respected_and_recent_ids_are_kept() {
        let dedup = EventDeduplicator::new(4);
        for i in 0..4 {
            assert!(dedup.first_seen("message", &format!("m{i}")));
        }
        assert_eq!(dedup.len(), 4);

        assert!(dedup.first_seen("message", "m4"));
        assert!(dedup.len() <= 4);

        // The newest ids survive pruning.
        assert!(!dedup.first_seen("message", "m4"));
        assert!(!dedup.first_seen("message", "m3"));
        // The oldest ids were evicted and can be seen again.
        assert!(dedup.first_seen("message", "m0"));

        // The bound holds under sustained churn.
        for i in 0..100 {
            dedup.first_seen("tool", &format!("call_{i}"));
            assert!(dedup.len() <= 4);
        }
    }

    #[test]
    fn forget_allows_a_key_to_be_seen_again() {
        let dedup = EventDeduplicator::new(8);
        assert!(dedup.first_seen("tool", "call_1"));
        assert!(!dedup.first_seen("tool", "call_1"));

        dedup.forget("tool", "call_1");
        assert!(dedup.is_empty());

        assert!(dedup.first_seen("tool", "call_1"));
        assert!(!dedup.first_seen("tool", "call_1"));
    }

    #[test]
    fn zero_capacity_disables_deduplication() {
        let dedup = EventDeduplicator::new(0);
        assert!(dedup.first_seen("message", "m1"));
        assert!(dedup.first_seen("message", "m1"));
        assert!(dedup.is_empty());
    }
}

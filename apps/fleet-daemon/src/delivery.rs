//! Retry throttle for durable outbox delivery.
//!
//! Every durable frame carries its own outbox sequence. The admin acknowledges
//! each applied sequence, and the storage layer advances its contiguous cursor
//! only when there is no gap. If a live frame is lost (broadcast lag, socket
//! write failure, admin restart), the sequence is never acknowledged and a
//! replay sends it again. This tracker only prevents the replay loop from
//! resending the same pending frame on every tick.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Suppresses duplicate replays of a sequence for a short retry window.
#[derive(Debug)]
pub struct DurableSendTracker {
    retry_after: Duration,
    inner: Mutex<HashMap<u64, Instant>>,
}

impl DurableSendTracker {
    /// Create a tracker; `retry_after` is how long to wait before resending an
    /// unacknowledged sequence.
    pub fn new(retry_after: Duration) -> Self {
        Self {
            retry_after,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Return `true` when this sequence should be sent now, and record the
    /// attempt. Unacknowledged sequences become eligible again after the retry
    /// window.
    pub fn should_send(&self, sequence: u64) -> bool {
        let now = Instant::now();
        let mut sent = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = sent.get(&sequence) {
            if now.duration_since(*previous) < self.retry_after {
                return false;
            }
        }
        sent.insert(sequence, now);
        true
    }

    /// Forget attempts for sequences the admin has acknowledged.
    pub fn forget_through(&self, cursor: u64) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|sequence, _| *sequence > cursor);
    }

    /// Forget everything after a transport generation change, so a reconnect
    /// retries pending frames immediately.
    pub fn reset(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::DurableSendTracker;
    use std::time::Duration;

    #[test]
    fn suppresses_until_the_retry_window_elapses() {
        let tracker = DurableSendTracker::new(Duration::from_millis(50));
        assert!(tracker.should_send(1));
        assert!(!tracker.should_send(1));
        std::thread::sleep(Duration::from_millis(60));
        assert!(tracker.should_send(1));
    }

    #[test]
    fn acknowledgement_and_reset_release_attempts() {
        let tracker = DurableSendTracker::new(Duration::from_secs(60));
        assert!(tracker.should_send(1));
        assert!(tracker.should_send(2));
        tracker.forget_through(1);
        assert!(tracker.should_send(1));
        assert!(!tracker.should_send(2));
        tracker.reset();
        assert!(tracker.should_send(2));
    }
}

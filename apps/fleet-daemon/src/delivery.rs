//! Tracks which durable outbox sequences have been handed to the transport.
//!
//! Events may be sent live before older unacknowledged rows are replayed. The
//! admin must only acknowledge a contiguous prefix, otherwise an early ack
//! could prune events that were never delivered. Every durable frame therefore
//! carries this watermark rather than its own sequence.

use std::collections::BTreeSet;
use std::sync::Mutex;

/// Contiguous delivery watermark shared by the live and replay senders.
#[derive(Debug, Default)]
pub struct OutboxDelivery {
    inner: Mutex<DeliveryState>,
}

#[derive(Debug, Default)]
struct DeliveryState {
    base: u64,
    sent: BTreeSet<u64>,
}

impl OutboxDelivery {
    /// Start tracking from a previously acknowledged cursor.
    pub fn new(base: u64) -> Self {
        Self {
            inner: Mutex::new(DeliveryState {
                base,
                sent: BTreeSet::new(),
            }),
        }
    }

    /// Mark one sequence as delivered and return the new contiguous watermark.
    pub fn mark(&self, sequence: u64) -> u64 {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if sequence <= state.base {
            return state.base;
        }
        state.sent.insert(sequence);
        loop {
            let next = state.base + 1;
            if !state.sent.remove(&next) {
                break;
            }
            state.base = next;
        }
        state.base
    }

    /// Start a new transport generation from the acknowledged cursor.
    ///
    /// Marks from a previous socket are discarded: frames queued but never
    /// written must not let a replay acknowledge past an undelivered event.
    pub fn reset(&self, base: u64) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.base = base;
        state.sent.clear();
    }

    /// Fold an acknowledged cursor into the tracker.
    pub fn ack(&self, cursor: u64) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cursor <= state.base {
            return;
        }
        state.base = cursor;
        state.sent = state.sent.split_off(&(cursor + 1));
    }

    /// Current contiguous watermark.
    #[cfg(test)]
    pub fn watermark(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .base
    }
}

#[cfg(test)]
mod tests {
    use super::OutboxDelivery;

    #[test]
    fn out_of_order_delivery_advances_only_a_contiguous_prefix() {
        let delivery = OutboxDelivery::new(0);
        assert_eq!(delivery.mark(2), 0);
        assert_eq!(delivery.mark(1), 2);
        assert_eq!(delivery.mark(4), 2);
        assert_eq!(delivery.mark(3), 4);
        delivery.ack(4);
        assert_eq!(delivery.watermark(), 4);
        assert_eq!(delivery.mark(3), 4);
    }

    #[test]
    fn reset_discards_marks_from_a_previous_connection() {
        let delivery = OutboxDelivery::new(0);
        assert_eq!(delivery.mark(1), 1);
        assert_eq!(delivery.mark(2), 2);
        delivery.reset(0);
        assert_eq!(delivery.mark(2), 0);
        assert_eq!(delivery.mark(1), 2);
    }

    #[test]
    fn older_acks_do_not_move_the_watermark_backwards() {
        let delivery = OutboxDelivery::new(5);
        assert_eq!(delivery.mark(6), 6);
        delivery.ack(4);
        assert_eq!(delivery.watermark(), 6);
    }
}

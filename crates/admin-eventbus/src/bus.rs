//! Broadcast event bus used to fan platform events out to subscribers.

use fleet_core_protocol::connection::PlatformEvent;
use tokio::sync::broadcast;

use crate::filter::{EventFilter, Subscription};

/// Default broadcast buffer capacity.
pub const DEFAULT_CAPACITY: usize = 4096;

/// Cloneable, in-process publish/subscribe bus for [`PlatformEvent`]s.
///
/// Publishing never blocks. A subscriber that cannot keep up is allowed to lag;
/// the gap is reported to that subscriber (and handled by
/// [`Subscription::recv`]) without affecting other subscribers.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<PlatformEvent>,
}

impl EventBus {
    /// Create a bus with the given buffer capacity.
    ///
    /// A capacity of zero is treated as one because Tokio's broadcast channel
    /// requires a non-zero capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    /// Subscribe to all published events.
    pub fn subscribe(&self) -> broadcast::Receiver<PlatformEvent> {
        self.sender.subscribe()
    }

    /// Subscribe with a filter, returning a helper that skips non-matching events.
    ///
    /// This is the intended entry point for the fleet-admin browser event
    /// WebSocket.
    pub fn subscribe_filtered(&self, filter: EventFilter) -> Subscription {
        Subscription::with_filter(self.subscribe(), filter)
    }

    /// Number of currently active receivers.
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Publish `event` and return the number of receivers it was made available to.
    ///
    /// If there are no subscribers the event is dropped and a debug log is
    /// emitted; this is not an error. Slow receivers do not block publishers.
    pub fn publish(&self, event: PlatformEvent) -> usize {
        if let Err(err) = self.sender.send(event) {
            let dropped = err.0;
            tracing::debug!(
                event_id = %dropped.event_id,
                event_type = %dropped.event_type,
                "no event bus subscribers; event dropped"
            );
        }
        self.sender.receiver_count()
    }

    /// Build a [`PlatformEvent`], publish it, and return it to the caller.
    ///
    /// `sequence` is left at its default (`0`); callers that need per-machine
    /// gap detection should set it on the event before publishing directly.
    pub fn publish_event(
        &self,
        event_type: &str,
        machine_id: Option<&str>,
        workspace_id: Option<&str>,
        session_id: Option<&str>,
        payload: serde_json::Value,
    ) -> PlatformEvent {
        let mut event = PlatformEvent::new(event_type, payload);
        event.machine_id = machine_id.map(str::to_string);
        event.workspace_id = workspace_id.map(str::to_string);
        event.session_id = session_id.map(str::to_string);
        self.publish(event.clone());
        event
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("receivers", &self.sender.receiver_count())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn publish_and_subscribe_receives_event() {
        let bus = EventBus::new(16);
        let mut receiver = bus.subscribe();

        let mut event = PlatformEvent::new("machine.status", json!({"status": "online"}));
        event.machine_id = Some("m1".to_string());

        let receivers = bus.publish(event.clone());
        assert_eq!(receivers, 1);

        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("receive should not time out")
            .expect("event should be delivered");
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn every_subscriber_receives_each_event() {
        let bus = EventBus::new(16);
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();

        let event = PlatformEvent::new("agent.event", json!({"n": 1}));
        assert_eq!(bus.publish(event.clone()), 2);

        let received_first = timeout(Duration::from_secs(1), first.recv())
            .await
            .expect("first should not time out")
            .expect("first receives");
        let received_second = timeout(Duration::from_secs(1), second.recv())
            .await
            .expect("second should not time out")
            .expect("second receives");

        assert_eq!(received_first, event);
        assert_eq!(received_second, event);
    }

    #[tokio::test]
    async fn publish_reports_zero_without_subscribers() {
        let bus = EventBus::new(4);
        let event = PlatformEvent::new("agent.event", json!({}));
        assert_eq!(bus.publish(event), 0);
        assert_eq!(bus.receiver_count(), 0);
    }

    #[tokio::test]
    async fn publish_event_sets_fields_and_returns_event() {
        let bus = EventBus::new(4);
        let mut receiver = bus.subscribe();

        let published = bus.publish_event(
            "agent.event",
            Some("m1"),
            Some("w1"),
            Some("s1"),
            json!({"kind": "message"}),
        );

        assert_eq!(published.event_type, "agent.event");
        assert_eq!(published.machine_id.as_deref(), Some("m1"));
        assert_eq!(published.workspace_id.as_deref(), Some("w1"));
        assert_eq!(published.session_id.as_deref(), Some("s1"));
        assert_eq!(published.payload, json!({"kind": "message"}));
        assert!(published.event_id.starts_with("evt_"));

        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("receive should not time out")
            .expect("event should be delivered");
        assert_eq!(received, published);
    }

    #[tokio::test]
    async fn filtered_subscription_skips_non_matching_events() {
        let bus = EventBus::new(16);
        let mut subscription = bus.subscribe_filtered(EventFilter::new().machine_id("m1"));

        bus.publish_event("agent.event", Some("m2"), None, None, json!({"n": 1}));
        bus.publish_event("agent.event", None, None, None, json!({"n": 2}));
        bus.publish_event("agent.event", Some("m1"), None, None, json!({"n": 3}));

        let received = timeout(Duration::from_secs(1), subscription.recv())
            .await
            .expect("receive should not time out")
            .expect("matching event should be delivered");
        assert_eq!(received.payload, json!({"n": 3}));
        assert_eq!(received.machine_id.as_deref(), Some("m1"));
    }

    #[tokio::test]
    async fn filtered_subscription_honours_type_prefix() {
        let bus = EventBus::new(16);
        let mut subscription =
            bus.subscribe_filtered(EventFilter::new().event_type_prefix("agent."));

        bus.publish_event("workspace.event", None, None, None, json!({}));
        let expected = bus.publish_event("agent.session", None, None, None, json!({}));

        let received = timeout(Duration::from_secs(1), subscription.recv())
            .await
            .expect("receive should not time out")
            .expect("matching event should be delivered");
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn lagged_receiver_does_not_panic_and_keeps_reading() {
        let bus = EventBus::new(2);
        let mut subscription = bus.subscribe_filtered(EventFilter::new().machine_id("m1"));

        for index in 0..64 {
            bus.publish_event(
                "agent.event",
                Some("m1"),
                None,
                None,
                json!({ "index": index }),
            );
        }

        let received = timeout(Duration::from_secs(1), subscription.recv())
            .await
            .expect("lagged receiver should still deliver a retained event")
            .expect("channel is still open");
        assert_eq!(received.event_type, "agent.event");
    }

    #[tokio::test]
    async fn subscription_ends_when_bus_is_dropped() {
        let bus = EventBus::new(4);
        let mut subscription = bus.subscribe_filtered(EventFilter::new());
        drop(bus);

        let outcome = timeout(Duration::from_secs(1), subscription.recv())
            .await
            .expect("closed channel should return promptly");
        assert!(outcome.is_none(), "closed subscription must yield None");
    }

    #[tokio::test]
    async fn zero_capacity_is_treated_as_one() {
        let bus = EventBus::new(0);
        let mut receiver = bus.subscribe();
        bus.publish_event("agent.event", None, None, None, json!({}));

        let received = timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("receive should not time out")
            .expect("event should be delivered");
        assert_eq!(received.event_type, "agent.event");
    }
}

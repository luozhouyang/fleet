//! Filtering subscriptions over the platform event stream.

use fleet_core_protocol::connection::PlatformEvent;
use tokio::sync::broadcast;

/// Declarative filter applied by [`Subscription`].
///
/// Every field is optional; `None` matches everything. When set, the
/// corresponding event field must be present and equal. `event_type_prefix`
/// matches with [`str::starts_with`], which lets callers such as the control
/// plane subscribe to families like `agent.`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFilter {
    /// Only accept events for this machine.
    pub machine_id: Option<String>,
    /// Only accept events for this workspace.
    pub workspace_id: Option<String>,
    /// Only accept events for this session.
    pub session_id: Option<String>,
    /// Only accept event types beginning with this prefix.
    pub event_type_prefix: Option<String>,
}

impl EventFilter {
    /// An empty filter that accepts every event.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict to a machine.
    pub fn machine_id(mut self, machine_id: impl Into<String>) -> Self {
        self.machine_id = Some(machine_id.into());
        self
    }

    /// Restrict to a workspace.
    pub fn workspace_id(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self
    }

    /// Restrict to a session.
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Restrict to event types with the given prefix.
    pub fn event_type_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.event_type_prefix = Some(prefix.into());
        self
    }

    /// Return `true` when `event` satisfies every configured constraint.
    pub fn matches(&self, event: &PlatformEvent) -> bool {
        field_matches(self.machine_id.as_deref(), event.machine_id.as_deref())
            && field_matches(self.workspace_id.as_deref(), event.workspace_id.as_deref())
            && field_matches(self.session_id.as_deref(), event.session_id.as_deref())
            && self
                .event_type_prefix
                .as_deref()
                .map(|prefix| event.event_type.starts_with(prefix))
                .unwrap_or(true)
    }
}

fn field_matches(expected: Option<&str>, actual: Option<&str>) -> bool {
    match expected {
        Some(expected) => actual == Some(expected),
        None => true,
    }
}

/// A filtered broadcast subscription.
///
/// [`Subscription::recv`] transparently skips events that do not match and
/// keeps going after [`broadcast::error::RecvError::Lagged`], so consumers can
/// treat it as a plain filtered stream.
pub struct Subscription {
    receiver: broadcast::Receiver<PlatformEvent>,
    filter: EventFilter,
}

impl Subscription {
    /// Wrap a broadcast receiver with an empty (match-all) filter.
    pub fn new(receiver: broadcast::Receiver<PlatformEvent>) -> Self {
        Self {
            receiver,
            filter: EventFilter::new(),
        }
    }

    /// Wrap a broadcast receiver with `filter`.
    pub fn with_filter(receiver: broadcast::Receiver<PlatformEvent>, filter: EventFilter) -> Self {
        Self { receiver, filter }
    }

    /// The filter currently applied by this subscription.
    pub fn filter(&self) -> &EventFilter {
        &self.filter
    }

    /// Replace the filter. The next call to [`Self::recv`] uses the new filter.
    pub fn set_filter(&mut self, filter: EventFilter) {
        self.filter = filter;
    }

    /// Consume the subscription and return the underlying broadcast receiver.
    pub fn into_inner(self) -> broadcast::Receiver<PlatformEvent> {
        self.receiver
    }

    /// Receive the next matching event.
    ///
    /// Non-matching events are discarded. If the receiver lagged behind, the
    /// gap is logged and reading continues; `None` is returned only once the
    /// channel is closed.
    pub async fn recv(&mut self) -> Option<PlatformEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    if self.filter.matches(&event) {
                        return Some(event);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        skipped,
                        "event subscription lagged; dropping retained events"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(event_type: &str, machine: Option<&str>, session: Option<&str>) -> PlatformEvent {
        let mut event = PlatformEvent::new(event_type, json!({}));
        event.machine_id = machine.map(str::to_string);
        event.session_id = session.map(str::to_string);
        event
    }

    #[test]
    fn empty_filter_matches_everything() {
        let filter = EventFilter::new();
        assert!(filter.matches(&event("agent.event", None, None)));
        assert!(filter.matches(&event("workspace.event", Some("m1"), Some("s1"))));
    }

    #[test]
    fn filter_requires_matching_fields() {
        let filter = EventFilter::new()
            .machine_id("m1")
            .workspace_id("w1")
            .session_id("s1")
            .event_type_prefix("agent.");

        let mut matching = event("agent.event", Some("m1"), Some("s1"));
        matching.workspace_id = Some("w1".to_string());
        assert!(filter.matches(&matching));

        assert!(!filter.matches(&event("agent.event", Some("m2"), Some("s1"))));
        assert!(!filter.matches(&event("agent.event", None, Some("s1"))));
        assert!(!filter.matches(&event("workspace.event", Some("m1"), Some("s1"))));
    }

    #[test]
    fn missing_event_field_does_not_match_set_filter() {
        let filter = EventFilter::new().machine_id("m1");
        assert!(!filter.matches(&event("agent.event", None, None)));
    }

    #[test]
    fn prefix_matching_is_case_sensitive_and_prefix_only() {
        let filter = EventFilter::new().event_type_prefix("agent.");
        assert!(filter.matches(&event("agent.event", None, None)));
        assert!(filter.matches(&event("agent.session", None, None)));
        assert!(!filter.matches(&event("workspace.event", None, None)));
        assert!(!filter.matches(&event("Agent.event", None, None)));
    }
}

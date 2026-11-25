//! Prometheus metrics for the fleet-admin process.
//!
//! Metrics are owned by an explicit [`Registry`] stored in [`AppState`] instead
//! of the process-global default registry, so tests and multiple admin
//! instances in one process stay isolated. The dedicated loopback listener
//! serves the rendered text; the public API and its routes are unchanged.
//!
//! Labels are deliberately bounded: HTTP routes come from the axum matched
//! route template and never from the raw request path, and machine, session and
//! user identifiers are not labels.

use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use prometheus::core::Collector;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};

use crate::state::AppState;

/// Content type of the Prometheus text exposition format.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Histogram buckets for HTTP request durations, in seconds.
const HTTP_DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Outcome of applying one durable agent event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableOutcome {
    /// The projection and the processed event id committed.
    Applied,
    /// The event id was already recorded; nothing was applied.
    Duplicate,
    /// Applying the projection failed; the event is not acknowledged.
    Failed,
}

impl DurableOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Duplicate => "duplicate",
            Self::Failed => "failed",
        }
    }
}

/// Collection of fleet-admin metrics.
///
/// Cloning is cheap: the clone shares the same underlying counters.
#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    http_requests: IntCounterVec,
    http_duration: HistogramVec,
    http_in_flight: IntGauge,
    websocket_connections: IntGauge,
    machines_online: IntGauge,
    event_subscribers: IntGauge,
    event_bus_dropped: IntCounter,
    durable_applied: IntCounter,
    durable_duplicate: IntCounter,
    durable_failed: IntCounter,
    unresolved_events: IntCounter,
    ack_batches: IntCounter,
    ack_sequences: IntCounter,
    gateway_resyncs: IntCounter,
}

impl Metrics {
    /// Create a registry and register every fleet-admin metric on it.
    ///
    /// Registration can only fail if a metric name is duplicated; the returned
    /// error is propagated so a programming mistake fails startup clearly
    /// instead of panicking.
    pub fn new() -> anyhow::Result<Self> {
        let registry = Registry::new();

        let http_requests = register(
            &registry,
            IntCounterVec::new(
                Opts::new(
                    "fleet_admin_http_requests_total",
                    "HTTP requests handled by fleet-admin.",
                ),
                &["method", "route", "status"],
            )?,
        )?;
        let http_duration = register(
            &registry,
            HistogramVec::new(
                HistogramOpts::new(
                    "fleet_admin_http_request_duration_seconds",
                    "HTTP request latency in seconds.",
                )
                .buckets(HTTP_DURATION_BUCKETS.to_vec()),
                &["method", "route"],
            )?,
        )?;
        let http_in_flight = register(
            &registry,
            IntGauge::new(
                "fleet_admin_http_requests_in_flight",
                "HTTP requests currently being served.",
            )?,
        )?;
        let websocket_connections = register(
            &registry,
            IntGauge::new(
                "fleet_admin_websocket_connections",
                "Open browser event WebSocket connections.",
            )?,
        )?;
        let machines_online = register(
            &registry,
            IntGauge::new(
                "fleet_admin_machines_online",
                "Machines with a live gateway connection.",
            )?,
        )?;
        let event_subscribers = register(
            &registry,
            IntGauge::new(
                "fleet_admin_event_bus_subscribers",
                "Active platform event bus subscribers.",
            )?,
        )?;
        let event_bus_dropped = register(
            &registry,
            IntCounter::new(
                "fleet_admin_event_bus_dropped_total",
                "Platform events dropped because a subscriber lagged.",
            )?,
        )?;

        // A single named vector keeps the required `outcome` label while each
        // outcome gets its own pre-registered counter handle.
        let durable_events = register(
            &registry,
            IntCounterVec::new(
                Opts::new(
                    "fleet_admin_durable_events_total",
                    "Durable agent events processed by outcome.",
                ),
                &["outcome"],
            )?,
        )?;
        let durable_applied = durable_events.with_label_values(&[DurableOutcome::Applied.label()]);
        let durable_duplicate =
            durable_events.with_label_values(&[DurableOutcome::Duplicate.label()]);
        let durable_failed = durable_events.with_label_values(&[DurableOutcome::Failed.label()]);

        let unresolved_events = register(
            &registry,
            IntCounter::new(
                "fleet_admin_unresolved_session_events_total",
                "Deferred durable events dropped because their session never resolved.",
            )?,
        )?;
        let ack_batches = register(
            &registry,
            IntCounter::new(
                "fleet_admin_ack_batches_total",
                "Batch acknowledgements sent to daemons.",
            )?,
        )?;
        let ack_sequences = register(
            &registry,
            IntCounter::new(
                "fleet_admin_ack_sequences_total",
                "Durable event sequences confirmed in batch acknowledgements.",
            )?,
        )?;
        let gateway_resyncs = register(
            &registry,
            IntCounter::new(
                "fleet_admin_gateway_resyncs_total",
                "Machine registry reconciliations after a gateway lag or periodic check.",
            )?,
        )?;

        let build_info = register(
            &registry,
            prometheus::GaugeVec::new(
                Opts::new(
                    "fleet_admin_build_info",
                    "fleet-admin build information; always 1.",
                ),
                &["version"],
            )?,
        )?;
        build_info
            .with_label_values(&[env!("CARGO_PKG_VERSION")])
            .set(1.0);

        Ok(Self {
            registry,
            http_requests,
            http_duration,
            http_in_flight,
            websocket_connections,
            machines_online,
            event_subscribers,
            event_bus_dropped,
            durable_applied,
            durable_duplicate,
            durable_failed,
            unresolved_events,
            ack_batches,
            ack_sequences,
            gateway_resyncs,
        })
    }

    /// Render the registry in the Prometheus text exposition format.
    pub fn render(&self) -> (String, &'static str) {
        let encoder = TextEncoder::new();
        let mut buffer = Vec::new();
        if let Err(err) = encoder.encode(&self.registry.gather(), &mut buffer) {
            tracing::error!(error = %err, "failed to encode admin metrics");
            return (String::new(), CONTENT_TYPE);
        }
        (String::from_utf8_lossy(&buffer).into_owned(), CONTENT_TYPE)
    }

    /// Record one completed HTTP request.
    pub fn observe_http(&self, method: &str, route: &str, status: u16, duration: Duration) {
        self.http_requests
            .with_label_values(&[method, route, &status.to_string()])
            .inc();
        self.http_duration
            .with_label_values(&[method, route])
            .observe(duration.as_secs_f64());
    }

    /// Increment the in-flight request gauge and return an RAII guard that
    /// decrements it when the request finishes.
    pub fn http_in_flight_guard(&self) -> InFlightGuard {
        self.http_in_flight.inc();
        InFlightGuard {
            metrics: self.clone(),
        }
    }

    /// Increment the open browser WebSocket gauge.
    pub fn inc_websocket_connections(&self) {
        self.websocket_connections.inc();
    }

    /// Decrement the open browser WebSocket gauge.
    pub fn dec_websocket_connections(&self) {
        self.websocket_connections.dec();
    }

    /// Track an open browser WebSocket with a drop guard.
    pub fn websocket_connection_guard(&self) -> WebSocketConnectionGuard {
        self.inc_websocket_connections();
        WebSocketConnectionGuard {
            metrics: self.clone(),
        }
    }

    /// Set the number of machines with a live connection.
    pub fn set_machines_online(&self, machines: usize) {
        self.machines_online.set(usize_to_i64(machines));
    }

    /// Set the number of platform event bus subscribers.
    pub fn set_event_subscribers(&self, subscribers: usize) {
        self.event_subscribers.set(usize_to_i64(subscribers));
    }

    /// Record events the event bus could not retain for a lagging subscriber.
    pub fn record_event_bus_dropped(&self, dropped: u64) {
        if dropped > 0 {
            self.event_bus_dropped.inc_by(dropped);
        }
    }

    /// Record the outcome of applying one durable agent event.
    pub fn record_durable(&self, outcome: DurableOutcome) {
        match outcome {
            DurableOutcome::Applied => self.durable_applied.inc(),
            DurableOutcome::Duplicate => self.durable_duplicate.inc(),
            DurableOutcome::Failed => self.durable_failed.inc(),
        }
    }

    /// Record durable events dropped after their session never resolved.
    pub fn record_unresolved_events(&self, count: usize) {
        if count > 0 {
            self.unresolved_events.inc_by(count as u64);
        }
    }

    /// Record one batch acknowledgement and its covered sequence count.
    pub fn record_ack_batch(&self, sequences: usize) {
        if sequences == 0 {
            return;
        }
        self.ack_batches.inc();
        self.ack_sequences.inc_by(sequences as u64);
    }

    /// Record one machine-registry reconciliation.
    pub fn record_gateway_resync(&self) {
        self.gateway_resyncs.inc();
    }
}

/// Decrements the in-flight request gauge when dropped.
pub struct InFlightGuard {
    metrics: Metrics,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.metrics.http_in_flight.dec();
    }
}

/// Decrements the WebSocket connection gauge when dropped.
pub struct WebSocketConnectionGuard {
    metrics: Metrics,
}

impl Drop for WebSocketConnectionGuard {
    fn drop(&mut self) {
        self.metrics.dec_websocket_connections();
    }
}

/// Router serving `/metrics` on the dedicated loopback listener.
pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/metrics", axum::routing::get(scrape))
        .with_state(state)
}

/// Refresh the aggregate gauges and render the current registry.
async fn scrape(State(state): State<AppState>) -> Response {
    state.refresh_metrics();
    let (body, content_type) = state.metrics.render();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
        body,
    )
        .into_response()
}

fn register<T>(registry: &Registry, metric: T) -> anyhow::Result<T>
where
    T: Collector + Clone + 'static,
{
    registry.register(Box::new(metric.clone()))?;
    Ok(metric)
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use fleet_admin_storage::AdminStorage;
    use fleet_core_config::AdminConfig;
    use tower::ServiceExt;

    use super::*;

    fn test_metrics() -> Metrics {
        Metrics::new().expect("metrics register")
    }

    async fn test_state() -> AppState {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        AppState::new(db, AdminConfig::default())
            .await
            .expect("state")
    }

    #[test]
    fn registers_every_metric_and_renders_text() {
        let metrics = test_metrics();
        metrics.observe_http("GET", "/api/v1/health", 200, Duration::from_millis(5));
        metrics.inc_websocket_connections();
        metrics.dec_websocket_connections();
        metrics.set_machines_online(2);
        metrics.set_event_subscribers(1);
        metrics.record_event_bus_dropped(3);
        metrics.record_durable(DurableOutcome::Applied);
        metrics.record_durable(DurableOutcome::Duplicate);
        metrics.record_durable(DurableOutcome::Failed);
        metrics.record_unresolved_events(2);
        metrics.record_ack_batch(4);
        metrics.record_gateway_resync();

        let (text, content_type) = metrics.render();
        assert_eq!(content_type, CONTENT_TYPE);
        for name in [
            "fleet_admin_http_requests_total",
            "fleet_admin_http_request_duration_seconds",
            "fleet_admin_http_requests_in_flight",
            "fleet_admin_websocket_connections",
            "fleet_admin_machines_online",
            "fleet_admin_event_bus_subscribers",
            "fleet_admin_event_bus_dropped_total",
            "fleet_admin_durable_events_total",
            "fleet_admin_unresolved_session_events_total",
            "fleet_admin_ack_batches_total",
            "fleet_admin_ack_sequences_total",
            "fleet_admin_gateway_resyncs_total",
            "fleet_admin_build_info",
        ] {
            assert!(text.contains(name), "missing metric {name}:\n{text}");
        }
        assert!(
            text.contains(
                "fleet_admin_http_requests_total{method=\"GET\",route=\"/api/v1/health\",status=\"200\"} 1"
            ),
            "unexpected counter sample:\n{text}"
        );
        assert!(
            text.contains("fleet_admin_durable_events_total{outcome=\"failed\"} 1"),
            "unexpected durable sample:\n{text}"
        );
        assert!(
            text.contains("fleet_admin_event_bus_dropped_total 3"),
            "unexpected drop sample:\n{text}"
        );
        assert!(
            text.contains("fleet_admin_ack_batches_total 1")
                && text.contains("fleet_admin_ack_sequences_total 4"),
            "unexpected acknowledgement samples:\n{text}"
        );
        assert!(
            text.contains("fleet_admin_unresolved_session_events_total 2"),
            "unexpected unresolved sample:\n{text}"
        );
    }

    #[test]
    fn websocket_guard_tracks_open_connections_across_drops() {
        let metrics = test_metrics();
        {
            let _first = metrics.websocket_connection_guard();
            let _second = metrics.websocket_connection_guard();
            let (text, _) = metrics.render();
            assert!(text.contains("fleet_admin_websocket_connections 2"));
        }
        let (text, _) = metrics.render();
        assert!(text.contains("fleet_admin_websocket_connections 0"));
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_prometheus_text() {
        let state = test_state().await;
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("metrics response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some(CONTENT_TYPE)
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("fleet_admin_build_info"));
        assert!(text.contains("fleet_admin_http_requests_in_flight 0"));
    }

    #[tokio::test]
    async fn metrics_endpoint_refreshes_aggregate_gauges() {
        let state = test_state().await;
        state
            .machines
            .register_or_update(fleet_core_protocol::machine::MachineRegistration {
                machine_id: "m_metrics".into(),
                name: "metrics".into(),
                hostname: "host".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                daemon_version: "0.1.0".into(),
                protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
                public_key: "key".into(),
                providers: Vec::new(),
                workspaces: Vec::new(),
            })
            .await
            .unwrap();

        let _receiver = state.events.subscribe();
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("metrics response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        // The registry was populated, but the machine has no live gateway
        // connection, so the online gauge stays at zero.
        assert!(text.contains("fleet_admin_machines_online 0"), "{text}");
        assert!(
            text.contains("fleet_admin_event_bus_subscribers 1"),
            "{text}"
        );
    }
}

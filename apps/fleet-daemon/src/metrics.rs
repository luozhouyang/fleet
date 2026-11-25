//! Prometheus metrics rendered on the daemon's loopback health listener.
//!
//! The handler builds a fresh registry and gathers values from [`Services`]
//! at scrape time, so the endpoint is stateless and stays idempotent. The
//! pre-existing health JSON payload is unchanged.
//!
//! [`Services`]: crate::services::Services

use prometheus::{Encoder, Gauge, GaugeVec, IntCounter, Opts, Registry, TextEncoder};

/// Content type of the Prometheus text exposition format.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Values gathered from the daemon at one scrape.
pub struct Snapshot<'a> {
    /// Daemon crate version.
    pub version: &'a str,
    /// Negotiated protocol version.
    pub protocol: u16,
    /// Seconds since the daemon services started.
    pub uptime_seconds: u64,
    /// Sessions currently in an active state.
    pub active_sessions: usize,
    /// State-lane events waiting for dispatch.
    pub state_depth: usize,
    /// Delta-lane events waiting for dispatch.
    pub delta_depth: usize,
    /// Overflow-lane events waiting for dispatch.
    pub overflow_depth: usize,
    /// Ephemeral deltas dropped because the delta lane stayed full.
    pub dropped_ephemeral: u64,
    /// Durable events the store permanently rejected.
    pub rejected_events: u64,
    /// Events whose payload was truncated for transport.
    pub truncated_events: u64,
    /// Durable rows still waiting for an acknowledgement.
    pub outbox_depth: u64,
    /// Age in seconds of the oldest unacknowledged outbox row.
    pub outbox_oldest_age_seconds: u64,
    /// Whether the durable event outbox is failing to persist.
    pub storage_degraded: bool,
    /// Whether the delivery loop last observed a connected transport.
    pub connection_up: bool,
    /// Cumulative reconnects after the first connection.
    pub reconnects_total: u64,
}

/// Render the daemon metrics in the Prometheus text exposition format.
pub fn render(snapshot: &Snapshot<'_>) -> Result<String, prometheus::Error> {
    let registry = Registry::new();

    let build_info = GaugeVec::new(
        Opts::new(
            "fleet_daemon_build_info",
            "fleet-daemon build information; always 1.",
        ),
        &["version", "protocol"],
    )?;
    registry.register(Box::new(build_info.clone()))?;
    build_info
        .with_label_values(&[snapshot.version, &snapshot.protocol.to_string()])
        .set(1.0);

    let uptime = gauge(
        &registry,
        "fleet_daemon_uptime_seconds",
        "Seconds since the daemon services started.",
    )?;
    uptime.set(snapshot.uptime_seconds as f64);

    let active_sessions = gauge(
        &registry,
        "fleet_daemon_active_sessions",
        "Sessions currently in an active state.",
    )?;
    active_sessions.set(snapshot.active_sessions as f64);

    let queue_depth = GaugeVec::new(
        Opts::new(
            "fleet_daemon_event_queue_depth",
            "Events waiting for dispatch, by pipeline lane.",
        ),
        &["lane"],
    )?;
    registry.register(Box::new(queue_depth.clone()))?;
    queue_depth
        .with_label_values(&["state"])
        .set(snapshot.state_depth as f64);
    queue_depth
        .with_label_values(&["delta"])
        .set(snapshot.delta_depth as f64);
    queue_depth
        .with_label_values(&["overflow"])
        .set(snapshot.overflow_depth as f64);

    let dropped_ephemeral = gauge(
        &registry,
        "fleet_daemon_dropped_ephemeral",
        "Ephemeral deltas dropped because the delta lane stayed full.",
    )?;
    dropped_ephemeral.set(snapshot.dropped_ephemeral as f64);

    let rejected_events = gauge(
        &registry,
        "fleet_daemon_rejected_events",
        "Durable events the store permanently rejected.",
    )?;
    rejected_events.set(snapshot.rejected_events as f64);

    let truncated_events = gauge(
        &registry,
        "fleet_daemon_truncated_events",
        "Events whose payload was truncated for transport.",
    )?;
    truncated_events.set(snapshot.truncated_events as f64);

    let outbox_depth = gauge(
        &registry,
        "fleet_daemon_outbox_depth",
        "Durable rows still waiting for an acknowledgement.",
    )?;
    outbox_depth.set(snapshot.outbox_depth as f64);

    let outbox_age = gauge(
        &registry,
        "fleet_daemon_outbox_oldest_pending_age_seconds",
        "Age of the oldest unacknowledged outbox row.",
    )?;
    outbox_age.set(snapshot.outbox_oldest_age_seconds as f64);

    let storage_degraded = gauge(
        &registry,
        "fleet_daemon_storage_degraded",
        "Whether the durable event outbox is failing to persist (0 or 1).",
    )?;
    storage_degraded.set(if snapshot.storage_degraded { 1.0 } else { 0.0 });

    let connection_up = gauge(
        &registry,
        "fleet_daemon_connection_up",
        "Whether the delivery loop last observed a connected transport (0 or 1).",
    )?;
    connection_up.set(if snapshot.connection_up { 1.0 } else { 0.0 });

    let reconnects = IntCounter::new(
        "fleet_daemon_reconnects_total",
        "Transports reconnected after the first successful connection.",
    )?;
    registry.register(Box::new(reconnects.clone()))?;
    reconnects.inc_by(snapshot.reconnects_total);

    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    encoder.encode(&registry.gather(), &mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

fn gauge(registry: &Registry, name: &str, help: &str) -> Result<Gauge, prometheus::Error> {
    let gauge = Gauge::new(name, help)?;
    registry.register(Box::new(gauge.clone()))?;
    Ok(gauge)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot<'static> {
        Snapshot {
            version: "0.2.2",
            protocol: 1,
            uptime_seconds: 90,
            active_sessions: 3,
            state_depth: 4,
            delta_depth: 2,
            overflow_depth: 1,
            dropped_ephemeral: 7,
            rejected_events: 5,
            truncated_events: 2,
            outbox_depth: 9,
            outbox_oldest_age_seconds: 42,
            storage_degraded: true,
            connection_up: true,
            reconnects_total: 6,
        }
    }

    #[test]
    fn renders_every_daemon_metric() {
        let text = render(&snapshot()).expect("render metrics");
        for name in [
            "fleet_daemon_build_info",
            "fleet_daemon_uptime_seconds",
            "fleet_daemon_active_sessions",
            "fleet_daemon_event_queue_depth",
            "fleet_daemon_dropped_ephemeral",
            "fleet_daemon_rejected_events",
            "fleet_daemon_truncated_events",
            "fleet_daemon_outbox_depth",
            "fleet_daemon_outbox_oldest_pending_age_seconds",
            "fleet_daemon_storage_degraded",
            "fleet_daemon_connection_up",
            "fleet_daemon_reconnects_total",
        ] {
            assert!(text.contains(name), "missing metric {name}:\n{text}");
        }
        assert!(
            text.contains("fleet_daemon_build_info{protocol=\"1\",version=\"0.2.2\"} 1"),
            "{text}"
        );
        assert!(text.contains("fleet_daemon_uptime_seconds 90"), "{text}");
        assert!(text.contains("fleet_daemon_active_sessions 3"), "{text}");
        assert!(
            text.contains("fleet_daemon_event_queue_depth{lane=\"delta\"} 2"),
            "{text}"
        );
        assert!(
            text.contains("fleet_daemon_event_queue_depth{lane=\"overflow\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("fleet_daemon_event_queue_depth{lane=\"state\"} 4"),
            "{text}"
        );
        assert!(text.contains("fleet_daemon_storage_degraded 1"), "{text}");
        assert!(text.contains("fleet_daemon_connection_up 1"), "{text}");
        assert!(text.contains("fleet_daemon_reconnects_total 6"), "{text}");
    }

    #[test]
    fn false_booleans_render_as_zero() {
        let mut snapshot = snapshot();
        snapshot.storage_degraded = false;
        snapshot.connection_up = false;
        let text = render(&snapshot).expect("render metrics");
        assert!(text.contains("fleet_daemon_storage_degraded 0"), "{text}");
        assert!(text.contains("fleet_daemon_connection_up 0"), "{text}");
    }
}

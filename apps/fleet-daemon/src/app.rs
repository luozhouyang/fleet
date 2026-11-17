use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use fleet_core_common::now;
use fleet_core_config::DaemonConfig;
use fleet_core_connection::{ClientConfig, ConnectionState, MachineConnectionClient};
use fleet_core_protocol::machine::HealthPayload;
use fleet_core_protocol::PROTOCOL_VERSION;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::delivery::DurableSendTracker;
use crate::events::{DaemonEventFanout, WatcherManager};
use crate::handler::DaemonHandler;
use crate::services::Services;

/// Run the daemon until interrupted.
pub async fn serve(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = DaemonConfig::load(config_path.as_deref())?;
    let services = Arc::new(Services::build(config).await?);
    let shutdown = CancellationToken::new();
    let fanout = DaemonEventFanout::new();
    let delivery = Arc::new(DurableSendTracker::new(Duration::from_secs(2)));
    let watchers = WatcherManager::new(services.clone(), fanout.clone(), shutdown.clone());
    let handler = Arc::new(DaemonHandler::new(services.clone(), watchers.clone()));

    tracing::info!(
        machine = %services.machine_id,
        providers = ?services.providers().iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
        "agent daemon starting"
    );

    spawn_runtime_event_forwarder(&services, fanout.clone(), shutdown.clone());

    let health_services = services.clone();
    let health_config = config_path;
    let health_shutdown = shutdown.clone();
    let health_task = tokio::spawn(async move {
        if let Err(err) = run_health_server(health_services, health_config, health_shutdown).await {
            tracing::error!(error = %err, "health server stopped");
        }
    });

    // Watch workspaces registered before this run.
    match services.workspaces.list(None).await {
        Ok(workspaces) => {
            for workspace in workspaces {
                watchers.ensure(workspace).await;
            }
        }
        Err(err) => tracing::warn!(error = %err, "could not list workspaces for watching"),
    }

    let mut connection_task = None;
    let server_url = services.config.connection.server_url.clone();

    if let Some(url) = server_url {
        let token_path = fleet_core_auth::machine_token_path(&services.config.machine.key_file);
        let token = load_token_from(&token_path)?.unwrap_or_default();
        let pairing_code =
            pairing_code_for(&token, services.config.connection.pairing_code.clone());
        let machine_name = services
            .config
            .machine
            .name
            .clone()
            .unwrap_or_else(|| services.machine_id.clone());
        let client_config = ClientConfig {
            server_url: url.clone(),
            token,
            machine_id: services.machine_id.clone(),
            machine_name,
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: PROTOCOL_VERSION,
            heartbeat_interval: Duration::from_millis(
                services.config.connection.heartbeat_interval_ms,
            ),
            reconnect_initial: Duration::from_millis(500),
            reconnect_max: Duration::from_millis(services.config.connection.reconnect_max_ms),
            handshake_timeout: Duration::from_secs(10),
            pairing_code,
            token_file: Some(token_path),
        };
        let client =
            MachineConnectionClient::new(client_config, services.identity.clone(), handler);
        let client = match services.previous_identity.clone() {
            Some(previous) => client.with_rotation_identity(previous),
            None => client,
        };
        spawn_live_event_forwarder(&fanout, delivery.clone(), client.clone(), shutdown.clone());
        spawn_durable_outbox_replay(
            services.storage.clone(),
            delivery.clone(),
            client.clone(),
            shutdown.clone(),
        );
        let run_client = client;
        let run_shutdown = shutdown.clone();
        connection_task = Some(tokio::spawn(async move {
            run_client.run(run_shutdown).await;
        }));
        tracing::info!(url = %url, "connecting to agent admin");
    } else {
        tracing::warn!("connection.server_url is not configured; running local-only");
    }

    shutdown_signal().await;
    tracing::info!("shutting down");
    shutdown.cancel();
    services.runtime.shutdown_all().await;
    if let Some(task) = connection_task {
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    }
    health_task.abort();
    Ok(())
}

fn load_token_from(path: &std::path::Path) -> anyhow::Result<Option<String>> {
    fleet_core_auth::load_machine_token(path).map_err(anyhow::Error::from)
}

/// A pairing code is only sent when the daemon has no per-machine token.
///
/// `DaemonConfig::apply_env` already trims `FLEET_PAIR_CODE` and maps an
/// empty value to `None`; this additionally guards against a lingering code in
/// the config after the machine has paired (the code is single-use, so sending
/// it with a valid token would make every restart fail).
fn pairing_code_for(token: &str, configured: Option<String>) -> Option<String> {
    if !token.is_empty() {
        return None;
    }
    configured
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty())
}

fn spawn_runtime_event_forwarder(
    services: &Arc<Services>,
    fanout: DaemonEventFanout,
    shutdown: CancellationToken,
) {
    let mut events = services.runtime.subscribe_processed();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => {
                    match event {
                        Ok(processed) => {
                            let payload = serde_json::to_value(&processed.event)
                                .unwrap_or(Value::Null);
                            match processed.cursor {
                                Some(sequence) => {
                                    fanout.send_with_cursor("agent.event", payload, sequence)
                                }
                                None => fanout.send("agent.event", payload),
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "agent event subscriber lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

fn spawn_live_event_forwarder(
    fanout: &DaemonEventFanout,
    delivery: Arc<DurableSendTracker>,
    client: Arc<MachineConnectionClient>,
    shutdown: CancellationToken,
) {
    let mut events = fanout.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => {
                    match event {
                        Ok((name, payload, durable)) => {
                            if client.state() != ConnectionState::Connected {
                                continue;
                            }
                            match durable {
                                Some(sequence) => {
                                    if delivery.should_send(sequence) {
                                        client.send_event_with_cursor(name, payload, sequence);
                                    }
                                }
                                None => client.send_event(name, payload),
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "live event forwarder lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

/// Replay durable events the admin has not acknowledged.
///
/// The cursor advances only over a contiguous acknowledged prefix. A gap (for
/// example a frame lost to broadcast lag or a disconnect) therefore keeps later
/// rows pending, and this task retries them until the admin applies and
/// acknowledges the missing sequence. Duplicate deliveries are expected and
/// deduplicated by the admin's `event_id`.
fn spawn_durable_outbox_replay(
    storage: Arc<fleet_daemon_storage::DaemonStorage>,
    delivery: Arc<DurableSendTracker>,
    client: Arc<MachineConnectionClient>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        let mut connection_id: Option<String> = None;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    if client.state() != fleet_core_connection::ConnectionState::Connected {
                        connection_id = None;
                        continue;
                    }
                    let current = client.connection_id();
                    if connection_id != current {
                        connection_id = current;
                        // A new socket must retry every pending event at once.
                        delivery.reset();
                    }
                    let cursor = storage.outbox.cursor().await.unwrap_or(0);
                    delivery.forget_through(cursor);
                    let pending = match storage.outbox.pending(512).await {
                        Ok(pending) => pending,
                        Err(error) => {
                            tracing::warn!(error = %error, "failed to read the durable event outbox");
                            continue;
                        }
                    };
                    for entry in pending {
                        if !delivery.should_send(entry.sequence) {
                            continue;
                        }
                        let payload = serde_json::to_value(&entry.event).unwrap_or(Value::Null);
                        client.send_event_with_cursor("agent.event", payload, entry.sequence);
                    }
                }
            }
        }
    });
}

async fn run_health_server(
    services: Arc<Services>,
    config_path: Option<PathBuf>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let addr = format!(
        "{}:{}",
        services.config.daemon.host, services.config.daemon.port
    );
    let app = health_router(HealthState {
        services,
        config_path,
    });

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "daemon health server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;
    Ok(())
}

/// State served by the local health endpoints.
#[derive(Clone)]
struct HealthState {
    services: Arc<Services>,
    /// Configuration file the daemon was asked to load, if any.
    config_path: Option<PathBuf>,
}

fn health_router(state: HealthState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .with_state(state)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "failed to listen for ctrl-c");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = fleet_core_common::shutdown::wait() => {}
    }
}

/// JSON returned by the local health endpoints.
///
/// The flattened payload keeps the original `/health` shape intact; the
/// readiness fields are additive and the status stays `200` so existing
/// liveness probes keep working.
#[derive(Debug, Serialize)]
struct HealthResponse {
    #[serde(flatten)]
    payload: HealthPayload,
    /// True when every deep check passes.
    ready: bool,
    /// Per-check outcome; failures carry a short reason.
    checks: Value,
}

async fn health(State(state): State<HealthState>) -> Json<HealthResponse> {
    let services = &state.services;
    let sessions = services.runtime.list_sessions().await;
    let active = sessions.iter().filter(|s| s.status.is_active()).count();
    let checks = readiness_checks(&services.config, state.config_path.as_deref());
    let ready = checks
        .as_object()
        .map(|checks| {
            checks
                .values()
                .all(|check| check["ok"].as_bool() == Some(true))
        })
        .unwrap_or(false);
    Json(HealthResponse {
        payload: HealthPayload {
            machine_id: services.machine_id.clone(),
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: PROTOCOL_VERSION,
            uptime_secs: services.started_at.elapsed().as_secs(),
            active_sessions: active as u32,
            timestamp: now(),
        },
        ready,
        checks,
    })
}

/// Collect the deep readiness checks without failing the endpoint itself.
fn readiness_checks(config: &DaemonConfig, config_path: Option<&Path>) -> Value {
    json!({
        "config": check_config(config_path),
        "key_file": check_key_file(&config.machine.key_file),
        "machine_token": check_machine_token(config),
    })
}

/// A configuration file is optional: without one the daemon uses built-in
/// defaults and environment overrides.
fn check_config(path: Option<&Path>) -> Value {
    let Some(path) = path else {
        return json!({"ok": true, "detail": "built-in defaults"});
    };
    match std::fs::read_to_string(path) {
        Ok(_) => json!({"ok": true}),
        Err(error) => {
            tracing::warn!(error = %error, "readiness: configuration file is unreadable");
            json!({"ok": false, "reason": "configuration file is unreadable"})
        }
    }
}

/// Re-read the key file so a deleted, corrupt or over-permissive file is
/// detected even though the daemon already holds the identity in memory.
fn check_key_file(path: &Path) -> Value {
    match fleet_core_auth::MachineIdentity::load(path) {
        Ok(_) => json!({"ok": true}),
        Err(error) => {
            tracing::warn!(error = %error, "readiness: machine key file is unreadable");
            json!({"ok": false, "reason": "machine key file is unreadable or invalid"})
        }
    }
}

/// A token is only required when the daemon is configured to reach an admin.
fn check_machine_token(config: &DaemonConfig) -> Value {
    let path = fleet_core_auth::machine_token_path(&config.machine.key_file);
    match fleet_core_auth::load_machine_token(&path) {
        Ok(Some(_)) => json!({"ok": true}),
        Ok(None) if config.connection.server_url.is_none() => {
            json!({"ok": true, "detail": "not required for local-only mode"})
        }
        Ok(None) => json!({"ok": false, "reason": "machine token is missing"}),
        Err(error) => {
            tracing::warn!(error = %error, "readiness: machine token is unreadable");
            json!({"ok": false, "reason": "machine token is unreadable"})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn health_state(dir: &Path, server_url: Option<&str>) -> HealthState {
        let mut config = DaemonConfig::default();
        config.machine.key_file = dir.join("machine.key");
        config.storage.database = dir.join("daemon.db");
        config.connection.server_url = server_url.map(ToString::to_string);
        HealthState {
            services: Arc::new(Services::build_local(config).await.unwrap()),
            config_path: None,
        }
    }

    async fn fetch_health(state: HealthState, uri: &str) -> (StatusCode, Value) {
        let response = health_router(state)
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn missing_token_file_means_no_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = fleet_core_auth::machine_token_path(&dir.path().join("machine.key"));
        assert_eq!(load_token_from(&path).unwrap(), None);
    }

    #[test]
    fn token_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = fleet_core_auth::machine_token_path(&dir.path().join("machine.key"));
        fleet_core_auth::save_machine_token(&path, "flm_abc").unwrap();
        assert_eq!(load_token_from(&path).unwrap().as_deref(), Some("flm_abc"));
    }

    #[test]
    fn a_lingering_pairing_code_is_ignored_once_a_token_exists() {
        assert_eq!(
            pairing_code_for("flm_abc", Some("ABCD-EFGH-JKMN".into())),
            None
        );
    }

    #[test]
    fn empty_or_whitespace_pairing_codes_are_absent() {
        assert_eq!(pairing_code_for("", Some(String::new())), None);
        assert_eq!(pairing_code_for("", Some("   \n".into())), None);
    }

    #[test]
    fn a_code_is_used_when_no_token_exists() {
        assert_eq!(
            pairing_code_for("", Some(" ABCD-EFGH-JKMN ".into())).as_deref(),
            Some("ABCD-EFGH-JKMN")
        );
    }

    #[tokio::test]
    async fn health_reports_ready_checks_with_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let state = health_state(dir.path(), Some("ws://127.0.0.1:1/ws/machine")).await;
        let token_path =
            fleet_core_auth::machine_token_path(&state.services.config.machine.key_file);
        fleet_core_auth::save_machine_token(&token_path, "flm_test").unwrap();

        let (status, body) = fetch_health(state, "/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ready"], true);
        assert_eq!(body["checks"]["config"]["ok"], true);
        assert_eq!(body["checks"]["key_file"]["ok"], true);
        assert_eq!(body["checks"]["machine_token"]["ok"], true);
        // The pre-existing shallow payload is still present.
        assert!(body["machine_id"].is_string());
        assert_eq!(body["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(body["active_sessions"], 0);
    }

    #[tokio::test]
    async fn health_reports_unreadable_config_key_and_token() {
        let dir = tempfile::tempdir().unwrap();
        let state = health_state(dir.path(), Some("ws://127.0.0.1:1/ws/machine")).await;
        std::fs::remove_file(&state.services.config.machine.key_file).unwrap();
        let state = HealthState {
            config_path: Some(dir.path().join("missing.toml")),
            ..state
        };

        let (status, body) = fetch_health(state, "/api/v1/health").await;
        assert_eq!(status, StatusCode::OK, "the shallow contract stays 200");
        assert_eq!(body["ready"], false);
        assert_eq!(body["checks"]["config"]["ok"], false);
        assert_eq!(body["checks"]["key_file"]["ok"], false);
        assert_eq!(body["checks"]["machine_token"]["ok"], false);
        assert_eq!(
            body["checks"]["machine_token"]["reason"],
            "machine token is missing"
        );
    }

    #[tokio::test]
    async fn health_is_ready_locally_without_a_machine_token() {
        let dir = tempfile::tempdir().unwrap();
        let state = health_state(dir.path(), None).await;

        let (status, body) = fetch_health(state, "/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ready"], true);
        assert_eq!(body["checks"]["machine_token"]["ok"], true);
        assert_eq!(
            body["checks"]["machine_token"]["detail"],
            "not required for local-only mode"
        );
    }
}

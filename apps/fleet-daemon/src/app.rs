use std::path::PathBuf;
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
use serde_json::Value;
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
    let health_shutdown = shutdown.clone();
    let health_task = tokio::spawn(async move {
        if let Err(err) = run_health_server(health_services, health_shutdown).await {
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
    let token = services.config.connection.token.clone();
    match (server_url, token) {
        (Some(url), Some(token)) => {
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
            let run_client = client.clone();
            let run_shutdown = shutdown.clone();
            connection_task = Some(tokio::spawn(async move {
                run_client.run(run_shutdown).await;
            }));
            tracing::info!(url = %url, "connecting to agent admin");
        }
        _ => {
            tracing::warn!(
                "AGENT_FLEET_SERVER_URL / AGENT_FLEET_TOKEN not configured; running local-only"
            );
        }
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
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .with_state(services.clone());

    let addr = format!(
        "{}:{}",
        services.config.daemon.host, services.config.daemon.port
    );
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "daemon health server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;
    Ok(())
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
    }
}

async fn health(State(services): State<Arc<Services>>) -> Json<HealthPayload> {
    let sessions = services.runtime.list_sessions().await;
    let active = sessions.iter().filter(|s| s.status.is_active()).count();
    Json(HealthPayload {
        machine_id: services.machine_id.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").into(),
        protocol_version: PROTOCOL_VERSION,
        uptime_secs: services.started_at.elapsed().as_secs(),
        active_sessions: active as u32,
        timestamp: now(),
    })
}

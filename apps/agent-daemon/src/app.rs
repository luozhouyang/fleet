use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_fleet_common::now;
use agent_fleet_config::DaemonConfig;
use agent_fleet_connection_gateway::{ClientConfig, MachineConnectionClient};
use agent_fleet_protocol::machine::HealthPayload;
use agent_fleet_protocol::PROTOCOL_VERSION;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::events::{EventOutbox, WatcherManager};
use crate::handler::DaemonHandler;
use crate::services::Services;

/// Run the daemon until interrupted.
pub async fn serve(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = DaemonConfig::load(config_path.as_deref())?;
    let services = Arc::new(Services::build(config).await?);
    let shutdown = CancellationToken::new();
    let outbox = EventOutbox::new();
    let watchers = WatcherManager::new(services.clone(), outbox.clone(), shutdown.clone());
    let handler = Arc::new(DaemonHandler::new(services.clone(), watchers.clone()));

    tracing::info!(
        machine = %services.machine_id,
        providers = ?services.providers().iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
        "agent daemon starting"
    );

    spawn_runtime_event_forwarder(&services, outbox.clone(), shutdown.clone());

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
            spawn_outbox_forwarder(&outbox, client.clone(), shutdown.clone());
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

    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %err, "failed to listen for ctrl-c");
    }
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
    outbox: EventOutbox,
    shutdown: CancellationToken,
) {
    let mut events = services.runtime.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => {
                    match event {
                        Ok(event) => {
                            let payload = serde_json::to_value(&event)
                                .unwrap_or(Value::Null);
                            outbox.send("agent.event", payload);
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

fn spawn_outbox_forwarder(
    outbox: &EventOutbox,
    client: Arc<MachineConnectionClient>,
    shutdown: CancellationToken,
) {
    let mut events = outbox.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => {
                    match event {
                        Ok((name, payload)) => client.send_event(name, payload),
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "outbox forwarder lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
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

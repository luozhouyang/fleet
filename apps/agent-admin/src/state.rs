//! Shared application state and the machine-key authenticator used by the
//! connection gateway.

use std::sync::Arc;
use std::time::Duration;

use agent_fleet_auth::TokenAuth;
use agent_fleet_common::{now, ApiError};
use agent_fleet_config::AdminConfig;
use agent_fleet_connection_gateway::{ConnectionGateway, ConnectionRegistry, MachineAuthenticator};
use agent_fleet_event_bus::EventBus;
use agent_fleet_machine_registry::{MachineRegistry, RegistryError};
use agent_fleet_protocol::machine::{Machine, MachineStatus};
use agent_fleet_protocol::PROTOCOL_VERSION;
use agent_fleet_storage::Database;
use async_trait::async_trait;

use crate::services::event_service::EventIndexes;

/// Everything a route or service needs, cheap to clone.
#[derive(Clone)]
pub struct AppState {
    /// Persistent storage shared with the machine registry.
    pub db: Arc<Database>,
    /// Persistent machine metadata plus its warm cache.
    pub machines: MachineRegistry,
    /// Machine WebSocket gateway.
    pub gateway: Arc<ConnectionGateway>,
    /// Live connection registry (same handle as the gateway's).
    pub connections: ConnectionRegistry,
    /// In-process platform event fan-out.
    pub events: EventBus,
    /// Admin bearer-token authenticator.
    pub auth: TokenAuth,
    /// Effective configuration.
    pub config: Arc<AdminConfig>,
    /// Indexes maintained by the machine-event pipeline.
    pub event_index: EventIndexes,
}

impl AppState {
    /// Build state from a database and effective configuration.
    ///
    /// The machine registry is warmed from storage and a gateway is created
    /// with the configured heartbeat policy.
    pub async fn new(db: Arc<Database>, config: AdminConfig) -> anyhow::Result<Self> {
        db.ensure_default_organization().await?;
        let machines = MachineRegistry::new(db.clone()).await?;
        let auth = TokenAuth::new(config.auth.admin_token.clone());
        let machine_token = match config.auth.machine_token.clone() {
            Some(token) => token,
            None => {
                tracing::warn!(
                    "auth.machine_token is not configured; the admin token is accepted on the machine WebSocket"
                );
                config.auth.admin_token.clone()
            }
        };
        let machine_auth = TokenAuth::new(machine_token);
        let authenticator = Arc::new(RegistryAuthenticator::new(machines.clone()));
        let gateway = ConnectionGateway::new(
            machine_auth,
            authenticator,
            Duration::from_millis(config.machines.heartbeat_interval_ms.max(1)),
            Duration::from_millis(config.machines.heartbeat_timeout_ms.max(1)),
        );
        let connections = gateway.registry();

        Ok(Self {
            db,
            machines,
            gateway,
            connections,
            events: EventBus::new(agent_fleet_event_bus::DEFAULT_CAPACITY),
            auth,
            config: Arc::new(config),
            event_index: EventIndexes::new(),
        })
    }

    /// Timeout applied to every proxied daemon request.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.config.machines.request_timeout_ms.max(1))
    }
}

/// [`MachineAuthenticator`] backed by the persistent machine registry.
///
/// The registry pins the first public key seen for a machine id; the gateway
/// consults this type during the Ed25519 challenge-response handshake.
pub struct RegistryAuthenticator {
    registry: MachineRegistry,
}

impl RegistryAuthenticator {
    /// Create an authenticator over `registry`.
    pub fn new(registry: MachineRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl MachineAuthenticator for RegistryAuthenticator {
    async fn pinned_key(&self, machine_id: &str) -> Result<Option<String>, ApiError> {
        self.registry
            .public_key(machine_id)
            .await
            .map_err(ApiError::from)
    }

    async fn pin_key(&self, machine_id: &str, public_key: &str) -> Result<(), ApiError> {
        match self.registry.set_public_key(machine_id, public_key).await {
            Ok(()) => Ok(()),
            // Trust on first use: the daemon proved possession of the key, so
            // create a placeholder row for a machine we have never seen. The
            // MachineConnected event fills in the remaining metadata.
            Err(RegistryError::NotFound(_)) => {
                let timestamp = now();
                let placeholder = Machine {
                    id: machine_id.to_string(),
                    name: machine_id.to_string(),
                    hostname: String::new(),
                    os: String::new(),
                    arch: String::new(),
                    daemon_version: String::new(),
                    protocol_version: PROTOCOL_VERSION,
                    status: MachineStatus::Unknown,
                    created_at: timestamp,
                    updated_at: timestamp,
                    connected_at: None,
                    last_seen_at: None,
                    providers: Vec::new(),
                    workspaces: Vec::new(),
                    workspace_count: 0,
                    public_key: Some(public_key.to_string()),
                };
                self.registry
                    .storage()
                    .upsert_machine(&placeholder)
                    .await
                    .map_err(ApiError::from)?;
                // Warm the registry cache so later updates do not re-read.
                self.registry
                    .get(machine_id)
                    .await
                    .map_err(ApiError::from)?;
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }
}

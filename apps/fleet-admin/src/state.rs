//! Shared application state and the machine-key authenticator used by the
//! connection gateway.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fleet_admin_eventbus::EventBus;
use fleet_admin_machine::{MachineRegistry, RegistryError};
use fleet_admin_storage::AdminStorage;
use fleet_core_auth::TokenAuth;
use fleet_core_common::{now, ApiError};
use fleet_core_config::AdminConfig;
use fleet_core_connection::{ConnectionGateway, ConnectionRegistry, MachineAuthenticator};
use fleet_core_protocol::machine::{Machine, MachineStatus};
use fleet_core_protocol::PROTOCOL_VERSION;

use crate::services::event_service::EventIndexes;

/// Everything a route or service needs, cheap to clone.
#[derive(Clone)]
pub struct AppState {
    /// Persistent storage shared with the machine registry.
    pub db: Arc<AdminStorage>,
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
    pub async fn new(db: Arc<AdminStorage>, config: AdminConfig) -> anyhow::Result<Self> {
        db.organizations.ensure_default().await?;
        let machines = MachineRegistry::new(db.clone()).await?;
        let auth = TokenAuth::new(config.auth.admin_token.clone());
        let machine_token = config
            .auth
            .machine_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "machine token is not configured; set AGENT_FLEET_MACHINE_TOKEN or auth.machine_token"
                )
            })?;
        if machine_token == config.auth.admin_token.trim() {
            anyhow::bail!("admin and machine tokens must differ");
        }
        let machine_auth = TokenAuth::new(machine_token.to_string());
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
            events: EventBus::new(fleet_admin_eventbus::DEFAULT_CAPACITY),
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

    async fn is_revoked(&self, machine_id: &str) -> Result<bool, ApiError> {
        self.registry
            .storage()
            .machines
            .is_revoked(machine_id)
            .await
            .map_err(ApiError::from)
    }

    async fn pin_key(&self, machine_id: &str, public_key: &str) -> Result<(), ApiError> {
        // A revocation can commit between the handshake pre-check and this
        // call. Re-check immediately before pinning/creating a placeholder so a
        // revoked machine cannot be resurrected by that race.
        if self
            .registry
            .storage()
            .machines
            .is_revoked(machine_id)
            .await
            .map_err(ApiError::from)?
        {
            return Err(ApiError::new(
                fleet_core_common::error_codes::FORBIDDEN,
                format!("machine {machine_id} has been revoked by an administrator"),
            ));
        }
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
                    .machines
                    .upsert(&placeholder)
                    .await
                    .map_err(ApiError::from)?;
                if self
                    .registry
                    .storage()
                    .machines
                    .is_revoked(machine_id)
                    .await
                    .map_err(ApiError::from)?
                {
                    // The revocation won the race after all; undo the row we
                    // just recreated.
                    let _ = self.registry.storage().machines.delete(machine_id).await;
                    return Err(ApiError::new(
                        fleet_core_common::error_codes::FORBIDDEN,
                        format!("machine {machine_id} has been revoked by an administrator"),
                    ));
                }
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

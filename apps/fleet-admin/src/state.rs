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

use crate::metrics::Metrics;
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
    /// Prometheus metrics for this process.
    pub metrics: Metrics,
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
        let authenticator: Arc<dyn MachineAuthenticator> = Arc::new(RegistryAuthenticator::new(
            machines.clone(),
            config.auth.pair_code_max_attempts,
        ));
        let gateway = ConnectionGateway::new(
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
            metrics: Metrics::new()?,
        })
    }

    /// Timeout applied to every proxied daemon request.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.config.machines.request_timeout_ms.max(1))
    }

    /// Refresh gauges that are derived from live administrative state.
    ///
    /// The metrics endpoint calls this immediately before rendering so a scrape
    /// reflects the current connections and subscribers rather than a stale
    /// value from the last event.
    pub fn refresh_metrics(&self) {
        self.metrics
            .set_machines_online(self.connections.online().len());
        self.metrics
            .set_event_subscribers(self.events.receiver_count());
    }
}

/// [`MachineAuthenticator`] backed by the persistent machine registry.
///
/// Pairing pins the public key for a machine id; the gateway consults this
/// type during the Ed25519 challenge-response handshake.
pub struct RegistryAuthenticator {
    registry: MachineRegistry,
    max_attempts: u32,
}

impl RegistryAuthenticator {
    /// Create an authenticator over `registry`.
    pub fn new(registry: MachineRegistry, max_attempts: u32) -> Self {
        Self {
            registry,
            max_attempts,
        }
    }
}

fn storage_error(error: fleet_admin_storage::StorageError) -> ApiError {
    ApiError::internal(error.to_string())
}

#[async_trait]
impl MachineAuthenticator for RegistryAuthenticator {
    async fn machine_state(
        &self,
        machine_id: &str,
    ) -> Result<fleet_core_protocol::pairing::MachineAuthState, ApiError> {
        self.registry
            .storage()
            .machines
            .machine_state(machine_id)
            .await
            .map_err(storage_error)
    }

    async fn machine_for_token(&self, token: &str) -> Result<Option<String>, ApiError> {
        self.registry
            .storage()
            .machines
            .by_token_hash(&fleet_core_auth::hash_token(token))
            .await
            .map_err(storage_error)
    }

    async fn pairing_code(
        &self,
        code: &str,
    ) -> Result<fleet_core_protocol::pairing::PairingCodeStatus, ApiError> {
        let Some(hash) = fleet_core_auth::hash_pair_code(code) else {
            return Ok(fleet_core_protocol::pairing::PairingCodeStatus::Invalid);
        };
        self.registry
            .storage()
            .pairing_codes
            .lookup(&hash, self.max_attempts)
            .await
            .map_err(storage_error)
    }

    async fn record_pairing_failure(&self, code_id: &str) -> Result<(), ApiError> {
        self.registry
            .storage()
            .pairing_codes
            .record_failure(code_id)
            .await
            .map_err(storage_error)
    }

    async fn complete_pairing(
        &self,
        request: fleet_core_protocol::pairing::PairingCompletion,
    ) -> Result<fleet_core_protocol::pairing::PairingOutcome, ApiError> {
        let machine_id = request.machine_id.clone();
        let outcome = self
            .registry
            .storage()
            .machines
            .complete_pairing(request, self.max_attempts)
            .await?;
        // Pairing writes storage directly. Refresh the warm cache so the next
        // heartbeat/registration observes the committed (possibly rotated) key
        // instead of writing the stale one back.
        self.registry
            .reload(&machine_id)
            .await
            .map_err(|err| ApiError::internal(err.to_string()))?;
        Ok(outcome)
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
            Ok(()) => {
                // A revocation can also commit after the pre-check but before
                // this pin. Re-check on the success path as well; a revoked
                // machine must not keep the key we just wrote.
                if self
                    .registry
                    .storage()
                    .machines
                    .is_revoked(machine_id)
                    .await
                    .map_err(ApiError::from)?
                {
                    let _ = self.registry.remove(machine_id).await;
                    return Err(ApiError::new(
                        fleet_core_common::error_codes::FORBIDDEN,
                        format!("machine {machine_id} has been revoked by an administrator"),
                    ));
                }
                Ok(())
            }
            // The daemon proved possession of the key during a rotation, so
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

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_common::now;
    use fleet_core_protocol::pairing::PairingCompletion;

    async fn create_code(db: &AdminStorage) -> String {
        db.pairing_codes
            .create(
                "org_default",
                "admin",
                None,
                now() + chrono::Duration::seconds(60),
                &fleet_core_common::new_id("codehash"),
            )
            .await
            .unwrap()
    }

    fn completion(
        code_id: String,
        public_key: &str,
        token_hash: &str,
        rotation_verified: bool,
    ) -> PairingCompletion {
        PairingCompletion {
            code_id,
            machine_id: "m1".into(),
            public_key: public_key.into(),
            token_hash: token_hash.into(),
            machine_name: "m1".into(),
            hostname: "host".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            daemon_version: "0.1.0".into(),
            protocol_version: 1,
            rotation_verified,
        }
    }

    #[tokio::test]
    async fn complete_pairing_refreshes_the_registry_cache_after_rotation() {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let registry = MachineRegistry::new(db.clone()).await.unwrap();
        let authenticator = RegistryAuthenticator::new(registry.clone(), 5);

        authenticator
            .complete_pairing(completion(
                create_code(&db).await,
                "key-a",
                "token-a",
                false,
            ))
            .await
            .unwrap();
        assert_eq!(
            registry.public_key("m1").await.unwrap().as_deref(),
            Some("key-a")
        );

        authenticator
            .complete_pairing(completion(create_code(&db).await, "key-b", "token-b", true))
            .await
            .unwrap();
        // Reading through the registry must observe the rotated key, not the
        // cached pre-rotation one.
        assert_eq!(
            registry.public_key("m1").await.unwrap().as_deref(),
            Some("key-b")
        );

        // A later heartbeat must not write the old key back.
        registry.touch("m1").await.unwrap();
        assert_eq!(
            db.machines
                .machine_state("m1")
                .await
                .unwrap()
                .pinned_key
                .as_deref(),
            Some("key-b")
        );
    }
}

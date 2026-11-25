use std::sync::Arc;

use dashmap::DashMap;
use fleet_admin_storage::AdminStorage;
use fleet_core_common::{now, MachineId, Timestamp};
use fleet_core_protocol::machine::{Machine, MachineRegistration, MachineStatus};

use crate::error::{RegistryError, Result};

/// Persistent machine metadata with a warm in-memory cache.
#[derive(Clone)]
pub struct MachineRegistry {
    storage: Arc<AdminStorage>,
    cache: Arc<DashMap<MachineId, Machine>>,
}

impl MachineRegistry {
    /// Load all known machines from storage into the cache.
    pub async fn new(storage: Arc<AdminStorage>) -> Result<Self> {
        let cache = Arc::new(DashMap::new());
        for machine in storage.machines.list().await? {
            cache.insert(machine.id.clone(), machine);
        }
        Ok(Self { storage, cache })
    }

    pub fn storage(&self) -> &Arc<AdminStorage> {
        &self.storage
    }

    /// Register a machine or refresh its metadata.
    ///
    /// The key was already authenticated and pinned by the gateway's pairing
    /// path; a differing key is rejected here as a defense-in-depth check.
    pub async fn register_or_update(&self, reg: MachineRegistration) -> Result<Machine> {
        if reg.machine_id.trim().is_empty() {
            return Err(RegistryError::InvalidRegistration(
                "machine_id must not be empty".into(),
            ));
        }
        if reg.public_key.trim().is_empty() {
            return Err(RegistryError::InvalidRegistration(
                "public_key must not be empty".into(),
            ));
        }

        let existing = match self.cache.get(&reg.machine_id) {
            Some(entry) => Some(entry.clone()),
            None => self.storage.machines.get(&reg.machine_id).await?,
        };

        if let Some(machine) = existing.as_ref() {
            if let Some(pinned) = machine.public_key.as_deref() {
                if pinned != reg.public_key {
                    return Err(RegistryError::KeyMismatch(reg.machine_id));
                }
            }
        }

        let timestamp = now();
        let machine = match existing {
            Some(mut machine) => {
                machine.name = reg.name.clone();
                machine.hostname = reg.hostname.clone();
                machine.os = reg.os.clone();
                machine.arch = reg.arch.clone();
                machine.daemon_version = reg.daemon_version.clone();
                machine.protocol_version = reg.protocol_version;
                machine.status = MachineStatus::Online;
                machine.connected_at = Some(timestamp);
                machine.last_seen_at = Some(timestamp);
                machine.providers = reg.providers.clone();
                machine.workspaces = reg.workspaces.clone();
                machine.workspace_count = reg.workspaces.len() as u32;
                machine.public_key = Some(reg.public_key.clone());
                machine.updated_at = timestamp;
                machine
            }
            None => Machine {
                id: reg.machine_id.clone(),
                name: reg.name.clone(),
                hostname: reg.hostname.clone(),
                os: reg.os.clone(),
                arch: reg.arch.clone(),
                daemon_version: reg.daemon_version.clone(),
                protocol_version: reg.protocol_version,
                status: MachineStatus::Online,
                created_at: timestamp,
                updated_at: timestamp,
                connected_at: Some(timestamp),
                last_seen_at: Some(timestamp),
                providers: reg.providers.clone(),
                workspaces: reg.workspaces.clone(),
                workspace_count: reg.workspaces.len() as u32,
                public_key: Some(reg.public_key.clone()),
            },
        };

        self.storage.machines.upsert(&machine).await?;
        self.cache.insert(machine.id.clone(), machine.clone());
        Ok(machine)
    }

    pub async fn set_online(&self, machine_id: &str) -> Result<Machine> {
        let at = now();
        self.storage
            .machines
            .update_status(machine_id, MachineStatus::Online, Some(at), Some(at))
            .await?;
        self.get_required(machine_id).await
    }

    pub async fn set_offline(&self, machine_id: &str) -> Result<Machine> {
        let at = now();
        self.storage
            .machines
            .update_status(machine_id, MachineStatus::Offline, None, Some(at))
            .await?;
        self.get_required(machine_id).await
    }

    /// Persist liveness only. This must never rewrite pairing-owned columns
    /// (`public_key`): the cached snapshot can be older than a completed
    /// pairing/rotation.
    pub async fn touch(&self, machine_id: &str) -> Result<()> {
        let status = match self.cache.get(machine_id) {
            Some(entry) => entry.status,
            None => {
                self.storage
                    .machines
                    .get(machine_id)
                    .await?
                    .ok_or_else(|| RegistryError::NotFound(machine_id.to_string()))?
                    .status
            }
        };
        let status = if status == MachineStatus::Unknown {
            MachineStatus::Online
        } else {
            status
        };
        let at = now();
        self.storage
            .machines
            .update_status(machine_id, status, None, Some(at))
            .await?;
        if let Some(mut entry) = self.cache.get_mut(machine_id) {
            entry.status = status;
            entry.last_seen_at = Some(at);
            entry.updated_at = at;
        }
        Ok(())
    }

    /// Record a heartbeat in the warm cache without a database write.
    ///
    /// Returns `false` when the machine is not cached, so the caller can fall
    /// back to the persistent [`Self::touch`]. The persisted `last_seen_at`
    /// trails the in-memory value by at most the caller's write interval.
    pub fn touch_cached(&self, machine_id: &str) -> bool {
        let Some(mut entry) = self.cache.get_mut(machine_id) else {
            return false;
        };
        let at = now();
        if entry.status == MachineStatus::Unknown {
            entry.status = MachineStatus::Online;
        }
        entry.last_seen_at = Some(at);
        entry.updated_at = at;
        true
    }

    pub async fn update_providers(
        &self,
        machine_id: &str,
        providers: Vec<fleet_core_protocol::agent::ProviderInfo>,
    ) -> Result<()> {
        self.storage
            .machines
            .update_providers(machine_id, &providers)
            .await?;
        if let Some(mut entry) = self.cache.get_mut(machine_id) {
            entry.providers = providers;
            entry.updated_at = now();
        }
        Ok(())
    }

    pub async fn update_workspaces(
        &self,
        machine_id: &str,
        workspaces: Vec<fleet_core_protocol::workspace::WorkspaceSummary>,
    ) -> Result<()> {
        self.storage
            .machines
            .update_workspaces(machine_id, &workspaces)
            .await?;
        if let Some(mut entry) = self.cache.get_mut(machine_id) {
            entry.workspace_count = workspaces.len() as u32;
            entry.workspaces = workspaces;
            entry.updated_at = now();
        }
        Ok(())
    }

    pub fn is_online(&self, machine_id: &str) -> bool {
        self.cache
            .get(machine_id)
            .map(|m| m.status.is_online())
            .unwrap_or(false)
    }

    pub fn get_cached(&self, machine_id: &str) -> Option<Machine> {
        self.cache.get(machine_id).map(|entry| entry.clone())
    }

    pub async fn get(&self, machine_id: &str) -> Result<Option<Machine>> {
        if let Some(machine) = self.get_cached(machine_id) {
            return Ok(Some(machine));
        }
        let machine = self.storage.machines.get(machine_id).await?;
        if let Some(machine) = machine.as_ref() {
            self.cache.insert(machine.id.clone(), machine.clone());
        }
        Ok(machine)
    }

    pub async fn list(&self) -> Result<Vec<Machine>> {
        let mut machines: Vec<Machine> = self.cache.iter().map(|e| e.value().clone()).collect();
        if machines.is_empty() {
            machines = self.storage.machines.list().await?;
            for machine in &machines {
                self.cache.insert(machine.id.clone(), machine.clone());
            }
        }
        machines.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(machines)
    }

    pub async fn public_key(&self, machine_id: &str) -> Result<Option<String>> {
        Ok(self
            .get(machine_id)
            .await?
            .and_then(|machine| machine.public_key))
    }

    pub async fn set_public_key(&self, machine_id: &str, public_key: &str) -> Result<()> {
        if self.storage.machines.get(machine_id).await?.is_none() {
            return Err(RegistryError::NotFound(machine_id.to_string()));
        }
        self.storage
            .machines
            .set_public_key(machine_id, public_key)
            .await?;
        self.reload(machine_id).await?;
        Ok(())
    }

    /// Re-read one machine from storage and update (or evict) the warm cache.
    ///
    /// Pairing and rotation write storage directly, so callers must reload the
    /// cache afterwards to avoid the next liveness update reverting the key.
    pub async fn reload(&self, machine_id: &str) -> Result<Option<Machine>> {
        let machine = self.storage.machines.get(machine_id).await?;
        match machine.as_ref() {
            Some(machine) => {
                self.cache.insert(machine.id.clone(), machine.clone());
            }
            None => {
                self.cache.remove(machine_id);
            }
        }
        Ok(machine)
    }

    async fn get_required(&self, machine_id: &str) -> Result<Machine> {
        self.reload(machine_id)
            .await?
            .ok_or_else(|| RegistryError::NotFound(machine_id.to_string()))
    }

    pub async fn remove(&self, machine_id: &str) -> Result<()> {
        self.storage.machines.delete(machine_id).await?;
        self.cache.remove(machine_id);
        Ok(())
    }

    /// Update last-seen for all currently online machines (used by the
    /// agent admin's periodic reconciliation).
    pub async fn touch_all_online(&self, older_than: Timestamp) -> Result<Vec<MachineId>> {
        let candidates: Vec<MachineId> = self
            .cache
            .iter()
            .filter(|entry| {
                entry.status.is_online()
                    && entry
                        .last_seen_at
                        .map(|seen| seen < older_than)
                        .unwrap_or(true)
            })
            .map(|entry| entry.id.clone())
            .collect();
        for id in &candidates {
            let _ = self.touch(id).await;
        }
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_protocol::agent::{AgentCapabilities, ProviderInfo, ProviderKind};

    async fn registry() -> MachineRegistry {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        MachineRegistry::new(db).await.unwrap()
    }

    fn registration(id: &str, key: &str) -> MachineRegistration {
        MachineRegistration {
            machine_id: id.into(),
            name: format!("test-{id}"),
            hostname: "host".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            daemon_version: "0.1.0".into(),
            protocol_version: 1,
            public_key: key.into(),
            providers: vec![ProviderInfo {
                id: "mock".into(),
                kind: ProviderKind::Mock,
                display_name: "Mock".into(),
                version: Some("1".into()),
                available: true,
                capabilities: AgentCapabilities::full(),
                models: vec![],
            }],
            workspaces: vec![],
        }
    }

    #[tokio::test]
    async fn register_and_list() {
        let registry = registry().await;
        let machine = registry
            .register_or_update(registration("m1", "key1"))
            .await
            .unwrap();
        assert_eq!(machine.status, MachineStatus::Online);
        assert_eq!(machine.providers.len(), 1);

        let machines = registry.list().await.unwrap();
        assert_eq!(machines.len(), 1);
        assert!(registry.is_online("m1"));
    }

    #[tokio::test]
    async fn rejects_key_change() {
        let registry = registry().await;
        registry
            .register_or_update(registration("m1", "key1"))
            .await
            .unwrap();
        let err = registry
            .register_or_update(registration("m1", "key2"))
            .await
            .unwrap_err();
        assert!(matches!(err, RegistryError::KeyMismatch(_)));
        assert_eq!(
            registry.public_key("m1").await.unwrap().as_deref(),
            Some("key1")
        );
    }

    #[tokio::test]
    async fn offline_and_remove() {
        let registry = registry().await;
        registry
            .register_or_update(registration("m1", "key1"))
            .await
            .unwrap();
        registry.set_offline("m1").await.unwrap();
        assert!(!registry.is_online("m1"));
        registry.remove("m1").await.unwrap();
        assert!(registry.get("m1").await.unwrap().is_none());
    }

    async fn pair(
        db: &AdminStorage,
        machine_id: &str,
        public_key: &str,
        token_hash: &str,
        rotation_verified: bool,
    ) {
        let code_id = db
            .pairing_codes
            .create(
                "org_default",
                "admin",
                None,
                now() + chrono::Duration::seconds(60),
                &fleet_core_common::new_id("codehash"),
            )
            .await
            .unwrap();
        db.machines
            .complete_pairing(
                fleet_core_protocol::pairing::PairingCompletion {
                    code_id,
                    machine_id: machine_id.to_string(),
                    public_key: public_key.to_string(),
                    token_hash: token_hash.to_string(),
                    machine_name: machine_id.to_string(),
                    hostname: "host".into(),
                    os: "linux".into(),
                    arch: "x86_64".into(),
                    daemon_version: "0.1.0".into(),
                    protocol_version: 1,
                    rotation_verified,
                },
                5,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn heartbeat_does_not_revert_a_rotated_key() {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let registry = MachineRegistry::new(db.clone()).await.unwrap();

        pair(&db, "m1", "key-a", "token-a", false).await;
        // Warm the cache with the pre-rotation key.
        assert_eq!(
            registry.public_key("m1").await.unwrap().as_deref(),
            Some("key-a")
        );

        // Rotate the key directly through storage, intentionally leaving the
        // cache stale (this is what pairing did before the cache refresh fix).
        pair(&db, "m1", "key-b", "token-b", true).await;

        // A heartbeat from the stale cache must not write key-a back.
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
        assert_eq!(
            db.machines
                .by_token_hash("token-b")
                .await
                .unwrap()
                .as_deref(),
            Some("m1")
        );
        assert_eq!(db.machines.by_token_hash("token-a").await.unwrap(), None);

        // A reload exposes the committed key.
        registry.reload("m1").await.unwrap();
        assert_eq!(
            registry.public_key("m1").await.unwrap().as_deref(),
            Some("key-b")
        );
    }

    #[tokio::test]
    async fn same_key_reconnect_keeps_created_at() {
        let registry = registry().await;
        let first = registry
            .register_or_update(registration("m1", "key1"))
            .await
            .unwrap();
        let second = registry
            .register_or_update(registration("m1", "key1"))
            .await
            .unwrap();
        assert_eq!(first.created_at, second.created_at);
        assert!(second.connected_at.unwrap() >= first.connected_at.unwrap());
    }
}

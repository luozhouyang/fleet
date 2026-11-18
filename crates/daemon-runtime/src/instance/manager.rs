//! Instance registry and lifecycle transitions.

use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;

use super::state::{AgentInstance, InstanceStatus};

/// Tracks agent instances and their session associations.
///
/// The manager is intentionally passive: it records lifecycle changes and
/// answers queries. Drivers own process and connection resources.
#[derive(Default)]
pub struct InstanceManager {
    instances: DashMap<String, Arc<parking_lot::RwLock<AgentInstance>>>,
    by_session: DashMap<String, String>,
}

impl InstanceManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new instance in the `Starting` state.
    pub fn create(&self, instance: AgentInstance) -> Arc<parking_lot::RwLock<AgentInstance>> {
        let id = instance.id.clone();
        let session_id = instance.session_id.clone();
        let record = Arc::new(parking_lot::RwLock::new(instance));
        self.instances.insert(id.clone(), Arc::clone(&record));
        self.by_session.insert(session_id, id);
        record
    }

    /// Look up an instance by id.
    pub fn get(&self, id: &str) -> Option<AgentInstance> {
        self.instances
            .get(id)
            .map(|entry| entry.value().read().clone())
    }

    /// Look up the instance serving a session.
    pub fn get_by_session(&self, session_id: &str) -> Option<AgentInstance> {
        let id = self.by_session.get(session_id)?.value().clone();
        self.get(&id)
    }

    /// Every instance, sorted by creation time then id.
    pub fn list(&self) -> Vec<AgentInstance> {
        let mut instances: Vec<_> = self
            .instances
            .iter()
            .map(|entry| entry.value().read().clone())
            .collect();
        instances.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        instances
    }

    /// Instances created from one definition.
    pub fn list_by_definition(&self, definition_id: &str) -> Vec<AgentInstance> {
        self.list()
            .into_iter()
            .filter(|instance| instance.definition_id == definition_id)
            .collect()
    }

    /// Mark an instance ready.
    pub fn mark_ready(&self, id: &str, process_id: Option<u32>, metadata: Value) {
        if let Some(entry) = self.instances.get(id) {
            entry.value().write().mark_ready(process_id, metadata);
        }
    }

    /// Mark an instance failed.
    pub fn mark_failed(&self, id: &str, metadata: Value) {
        if let Some(entry) = self.instances.get(id) {
            entry.value().write().mark_failed(metadata);
        }
    }

    /// Mark an instance busy.
    pub fn mark_busy(&self, id: &str) {
        if let Some(entry) = self.instances.get(id) {
            entry.value().write().status = InstanceStatus::Busy;
        }
    }

    /// Mark the instance serving `session_id` idle after a turn completes.
    pub fn mark_session_idle(&self, session_id: &str) {
        let Some(instance) = self.get_by_session(session_id) else {
            return;
        };
        if let Some(entry) = self.instances.get(&instance.id) {
            entry.value().write().mark_idle();
        }
    }

    /// Mark an instance stopping.
    pub fn mark_stopping(&self, id: &str) {
        if let Some(entry) = self.instances.get(id) {
            entry.value().write().status = InstanceStatus::Stopping;
        }
    }

    /// Mark an instance stopped and release its session association.
    pub fn stop(&self, id: &str) {
        if let Some((_, entry)) = self.instances.remove(id) {
            let mut instance = entry.write();
            instance.mark_stopped();
            self.by_session.remove(&instance.session_id);
        }
    }

    /// Remove an instance entirely.
    pub fn remove(&self, id: &str) -> Option<AgentInstance> {
        let (_, entry) = self.instances.remove(id)?;
        let instance = entry.read().clone();
        self.by_session.remove(&instance.session_id);
        Some(instance)
    }

    /// Remove the instance serving a session, if any.
    pub fn remove_by_session(&self, session_id: &str) -> Option<AgentInstance> {
        let id = self.by_session.remove(session_id)?.1;
        self.remove(&id)
    }

    /// Number of tracked instances.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether no instance is tracked.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_and_session_index() {
        let manager = InstanceManager::new();
        manager.create(AgentInstance::starting("i1", "def", "acp", "s1"));
        assert_eq!(manager.get_by_session("s1").unwrap().id, "i1");
        manager.mark_ready("i1", Some(42), serde_json::json!({"protocol": 2}));
        let instance = manager.get("i1").unwrap();
        assert_eq!(instance.status, InstanceStatus::Ready);
        assert_eq!(instance.process_id, Some(42));
        assert!(instance.started_at.is_some());
        manager.stop("i1");
        assert!(manager.get_by_session("s1").is_none());
        assert!(manager.is_empty());
    }
}

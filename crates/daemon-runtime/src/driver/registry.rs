//! The driver registry.
//!
//! The registry is built once at daemon startup. Drivers are never unloaded
//! while the daemon runs, so a session can never observe a driver disappear.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Result, RuntimeError};

use super::trait_def::{AgentDriver, DriverInfo};

/// Driver implementations keyed by id.
#[derive(Default)]
pub struct DriverRegistry {
    drivers: HashMap<String, Arc<dyn AgentDriver>>,
}

impl DriverRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a driver.
    ///
    /// Fails on duplicate ids: silently replacing a driver would make the
    /// behavior of existing definitions depend on registration order.
    pub fn register(&mut self, driver: Arc<dyn AgentDriver>) -> Result<()> {
        let id = driver.driver_id().to_string();
        if id.trim().is_empty() {
            return Err(RuntimeError::DefinitionInvalid(
                "driver id must not be empty".to_string(),
            ));
        }
        if self.drivers.contains_key(&id) {
            return Err(RuntimeError::DuplicateDriver(id));
        }
        self.drivers.insert(id, driver);
        Ok(())
    }

    /// Look up a driver by id.
    pub fn get(&self, id: &str) -> Option<Arc<dyn AgentDriver>> {
        self.drivers.get(id).cloned()
    }

    /// Whether a driver id is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.drivers.contains_key(id)
    }

    /// Descriptors for every driver, sorted by id.
    pub fn list(&self) -> Vec<DriverInfo> {
        let mut infos: Vec<_> = self.drivers.values().map(|driver| driver.info()).collect();
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    /// Number of registered drivers.
    pub fn len(&self) -> usize {
        self.drivers.len()
    }

    /// Whether no driver is registered.
    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::tests::StubDriver;

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut registry = DriverRegistry::new();
        registry.register(Arc::new(StubDriver::new("acp"))).unwrap();
        assert!(matches!(
            registry.register(Arc::new(StubDriver::new("acp"))),
            Err(RuntimeError::DuplicateDriver(_))
        ));
        assert!(registry.contains("acp"));
        assert_eq!(registry.list().len(), 1);
    }
}

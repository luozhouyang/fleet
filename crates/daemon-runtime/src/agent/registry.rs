//! Registry of known agent definitions.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Result, RuntimeError};

use super::definition::AgentDefinition;

/// A thread-safe collection of agent definitions keyed by id.
///
/// The registry is immutable once the daemon starts constructing sessions;
/// [`AgentRegistry::register`] rejects duplicate ids and [`AgentRegistry::update`]
/// is the explicit way to replace a definition. Updating a definition never
/// affects running instances because sessions persist a launch snapshot.
#[derive(Default)]
pub struct AgentRegistry {
    definitions: HashMap<String, Arc<AgentDefinition>>,
}

impl AgentRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a definition.
    ///
    /// Fails when the id is already present or the definition is invalid.
    pub fn register(&mut self, definition: AgentDefinition) -> Result<()> {
        definition
            .validate()
            .map_err(RuntimeError::DefinitionInvalid)?;
        if self.definitions.contains_key(&definition.id) {
            return Err(RuntimeError::Conflict(format!(
                "agent definition `{}` is already registered",
                definition.id
            )));
        }
        self.definitions
            .insert(definition.id.clone(), Arc::new(definition));
        Ok(())
    }

    /// Replace an existing definition.
    ///
    /// Running sessions are unaffected because they keep their own snapshot.
    pub fn update(&mut self, definition: AgentDefinition) -> Result<()> {
        definition
            .validate()
            .map_err(RuntimeError::DefinitionInvalid)?;
        if !self.definitions.contains_key(&definition.id) {
            return Err(RuntimeError::AgentNotFound(definition.id.clone()));
        }
        self.definitions
            .insert(definition.id.clone(), Arc::new(definition));
        Ok(())
    }

    /// Look up a definition by id.
    pub fn get(&self, id: &str) -> Option<Arc<AgentDefinition>> {
        self.definitions.get(id).cloned()
    }

    /// Every definition, sorted by id.
    pub fn list(&self) -> Vec<Arc<AgentDefinition>> {
        let mut definitions: Vec<_> = self.definitions.values().cloned().collect();
        definitions.sort_by(|a, b| a.id.cmp(&b.id));
        definitions
    }

    /// Remove a definition.
    pub fn remove(&mut self, id: &str) -> Result<Arc<AgentDefinition>> {
        self.definitions
            .remove(id)
            .ok_or_else(|| RuntimeError::AgentNotFound(id.to_string()))
    }

    /// Number of registered definitions.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Whether no definition is registered.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Return `true` when the id is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.definitions.contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::definition::ProcessLaunchConfig;

    fn definition(id: &str) -> AgentDefinition {
        AgentDefinition::process(id, id, "legacy", ProcessLaunchConfig::new("true"))
    }

    #[test]
    fn register_rejects_duplicates_and_update_replaces() {
        let mut registry = AgentRegistry::new();
        registry.register(definition("a")).unwrap();
        assert!(matches!(
            registry.register(definition("a")),
            Err(RuntimeError::Conflict(_))
        ));
        let mut updated = definition("a");
        updated.name = "renamed".into();
        registry.update(updated).unwrap();
        assert_eq!(registry.get("a").unwrap().name, "renamed");
        assert!(matches!(
            registry.update(definition("missing")),
            Err(RuntimeError::AgentNotFound(_))
        ));
        registry.remove("a").unwrap();
        assert!(registry.is_empty());
    }

    #[test]
    fn list_is_sorted() {
        let mut registry = AgentRegistry::new();
        registry.register(definition("b")).unwrap();
        registry.register(definition("a")).unwrap();
        let ids: Vec<String> = registry.list().iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }
}

//! Build the agent registry from built-in and user definitions.

use fleet_core_config::DaemonConfig;

use crate::agent::config::{builtin_definitions, merge_user_definitions};
use crate::agent::registry::AgentRegistry;
use crate::error::Result;

/// Build a registry containing the built-in agents plus `[agents.*]` overrides.
///
/// A malformed user definition is reported as an error so the daemon can log a
/// clear startup diagnostic; one bad agent must not make the daemon silently
/// drop every other agent.
pub fn build_agent_registry(config: &DaemonConfig) -> Result<AgentRegistry> {
    let definitions = merge_user_definitions(builtin_definitions(config), &config.agents)
        .map_err(crate::error::RuntimeError::DefinitionInvalid)?;
    let mut registry = AgentRegistry::new();
    for definition in definitions {
        registry.register(definition)?;
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_config::AgentDefinitionConfig;

    #[test]
    fn registry_contains_builtins_and_overrides() {
        let mut config = DaemonConfig::default();
        config.agents.insert(
            "custom".to_string(),
            AgentDefinitionConfig {
                name: Some("Custom".into()),
                ..AgentDefinitionConfig::default()
            },
        );
        let registry = build_agent_registry(&config).unwrap();
        assert!(registry.contains("claude"));
        assert!(registry.contains("claude-acp"));
        assert!(registry.contains("custom"));
        assert_eq!(registry.get("custom").unwrap().driver_id, "acp");
    }
}

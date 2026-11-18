//! Agent and driver catalog views.
//!
//! These types are the stable shape used by the daemon RPC, CLI and admin
//! APIs. They intentionally contain no ACP schema types.

use fleet_core_protocol::agent::AgentCapabilities;
use serde::{Deserialize, Serialize};

use super::definition::{AgentDefinitionSource, AgentLaunchConfig};

/// One agent definition as shown in the catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCatalogEntry {
    /// Agent definition id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Driver used to launch this agent.
    pub driver_id: String,
    /// Driver protocol family (`acp` or `native`).
    pub driver_protocol: String,
    /// Whether new sessions may use this agent.
    pub enabled: bool,
    /// Whether the executable exists and the agent can currently start.
    pub available: bool,
    /// Driver capabilities, or declared capabilities when the driver is absent.
    pub capabilities: AgentCapabilities,
    /// Definition source.
    pub source: AgentDefinitionSource,
    /// Executable name or path for process-launched agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
}

/// Build a catalog entry from a definition and its driver, if registered.
pub fn entry(
    definition: &super::definition::AgentDefinition,
    driver: Option<&std::sync::Arc<dyn crate::driver::AgentDriver>>,
) -> AgentCatalogEntry {
    let executable = match &definition.launch {
        AgentLaunchConfig::Process(process) => Some(process.executable.clone()),
        AgentLaunchConfig::Remote(_) => None,
    };
    let available = definition.enabled && executable_available(definition) && driver.is_some();
    AgentCatalogEntry {
        id: definition.id.clone(),
        name: definition.name.clone(),
        description: definition.description.clone(),
        driver_id: definition.driver_id.clone(),
        driver_protocol: driver
            .map(|driver| driver.info().protocol.as_str().to_string())
            .unwrap_or_else(|| "unregistered".to_string()),
        enabled: definition.enabled,
        available,
        capabilities: driver
            .map(|driver| driver.capabilities())
            .unwrap_or_else(|| definition.declared_capabilities.clone()),
        source: definition.source,
        executable,
    }
}

/// Whether the definition's executable is present.
///
/// In-process agents (mock) and definitions with an empty executable are
/// treated as available; their driver decides at start time.
pub fn executable_available(definition: &super::definition::AgentDefinition) -> bool {
    match &definition.launch {
        AgentLaunchConfig::Process(process) => {
            let executable = process.executable.as_str();
            if executable.is_empty() || executable == "mock" {
                return true;
            }
            crate::provider::binary_exists(executable)
        }
        AgentLaunchConfig::Remote(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ProcessLaunchConfig;

    #[test]
    fn catalog_entry_reports_unregistered_driver_as_unavailable() {
        let definition = crate::agent::AgentDefinition::process(
            "a",
            "A",
            "missing",
            ProcessLaunchConfig::new("true"),
        );
        let entry = entry(&definition, None);
        assert!(!entry.available);
        assert_eq!(entry.driver_protocol, "unregistered");
    }
}

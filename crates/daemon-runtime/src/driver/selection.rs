//! Driver resolution for an agent definition.

use std::sync::Arc;

use crate::agent::AgentDefinition;
use crate::error::{Result, RuntimeError};

use super::registry::DriverRegistry;
use super::trait_def::AgentDriver;

/// Resolve the driver that will serve `definition`.
///
/// The definition's `driver_id` is authoritative. A `fallback_driver` is used
/// only when the primary driver is not registered at all, and the fallback is
/// logged explicitly — Fleet never switches drivers silently and never
/// switches after a session has started.
pub fn resolve_driver(
    definition: &AgentDefinition,
    drivers: &DriverRegistry,
) -> Result<Arc<dyn AgentDriver>> {
    if !definition.enabled {
        return Err(RuntimeError::AgentDisabled(definition.id.clone()));
    }
    if let Some(driver) = drivers.get(&definition.driver_id) {
        return Ok(driver);
    }
    if let Some(fallback) = &definition.fallback_driver {
        if let Some(driver) = drivers.get(fallback) {
            tracing::warn!(
                agent = %definition.id,
                driver = %definition.driver_id,
                fallback = %fallback,
                "agent driver is not registered; using the configured fallback driver"
            );
            return Ok(driver);
        }
    }
    Err(RuntimeError::DriverNotFound {
        driver: definition.driver_id.clone(),
        agent: definition.id.clone(),
    })
}

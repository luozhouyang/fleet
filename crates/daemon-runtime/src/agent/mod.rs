//! Agent definition model and registry.
//!
//! An [`AgentDefinition`] describes an agent that *can* be launched. It is
//! deliberately separate from a [`crate::driver::AgentDriver`] (how to speak
//! its protocol) and from a running agent instance. One driver can serve many
//! definitions; one definition names exactly one driver.

pub mod catalog;
pub mod config;
pub mod definition;
pub mod discovery;
pub mod registry;

pub use catalog::AgentCatalogEntry;
pub use definition::{
    AgentDefinition, AgentDefinitionSource, AgentLaunchConfig, CwdPolicy, ProcessLaunchConfig,
    RemoteLaunchConfig,
};
pub use registry::AgentRegistry;

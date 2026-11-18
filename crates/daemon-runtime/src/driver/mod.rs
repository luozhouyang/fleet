//! The agent driver boundary.
//!
//! A driver owns protocol mechanics and runtime resources (processes,
//! connections). It never owns Fleet session state and never writes the
//! transcript: every observable change goes through
//! [`crate::event::AgentEventEmitter`].
//!
//! The same interface is implemented by the generic ACP driver and by the
//! native protocol drivers (Claude, Codex, OpenCode, Generic PTY, Mock).

pub mod acp;
pub mod capabilities;
pub mod context;
pub mod instance;
pub mod registry;
pub mod selection;
pub mod trait_def;

pub mod legacy;
pub mod native;

#[cfg(test)]
mod tests;

pub use acp::{AcpDriver, AcpDriverConfig, AcpProtocolPolicy, AcpProtocolVersion};
pub use capabilities::{CapabilityStatus, DetailedCapabilities, ResolvedCapabilities};
pub use context::DriverContext;
pub use instance::{AgentInstanceHandle, DriverStartResult};
pub use native::{
    ClaudeNativeDriver, CodexNativeDriver, GenericNativeDriver, MockNativeDriver,
    OpenCodeNativeDriver,
};
pub use registry::DriverRegistry;
pub use selection::resolve_driver;
pub use trait_def::{
    AgentDriver, AgentPrompt, DriverInfo, DriverProbeResult, DriverProtocol, ImageContent,
    PromptAcceptance, PromptContent, ResumeResult,
};

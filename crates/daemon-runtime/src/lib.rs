//! Agent provider abstraction, unified runtime and adapters.
//!
//! The crate defines the [`AgentProvider`] trait, the [`AgentRuntime`] session
//! manager and four adapters:
//!
//! - [`MockProvider`]: fully functional in-process provider used by E2E tests.
//! - [`ClaudeProvider`]: Claude Code CLI `stream-json` adapter.
//! - [`AcpProvider`]: Agent Client Protocol adapter (Claude Code via
//!   `claude-agent-acp`).
//! - [`CodexProvider`]: Codex app-server JSON-RPC adapter.
//! - [`GenericProvider`]: PTY adapter for terminal-compatible agents.
//!
//! Every adapter reports only the capabilities it genuinely implements. The
//! runtime owns session state transitions, permission decisions and event
//! fan-out; adapters emit normalized [`AgentEvent`](fleet_core_protocol::agent::AgentEvent)s
//! through [`AgentEventEmitter`].

#![warn(missing_docs)]

pub mod acp;
pub mod agent;
pub mod claude;
pub mod codex;
pub mod driver;
pub mod generic;
pub mod instance;
pub mod mock;
pub mod opencode;

mod cancel;
mod error;
mod event;
mod line_buffer;
mod provider;
mod runtime;
mod session;

use fleet_core_protocol::agent::AgentEvent;

pub use agent::{
    AgentDefinition, AgentDefinitionSource, AgentLaunchConfig, AgentRegistry, CwdPolicy,
    ProcessLaunchConfig, RemoteLaunchConfig,
};
pub use driver::{
    AcpDriver, AcpDriverConfig, AcpProtocolPolicy, AcpProtocolVersion, AgentDriver,
    AgentInstanceHandle, AgentPrompt, CapabilityStatus, DetailedCapabilities, DriverContext,
    DriverInfo, DriverProbeResult, DriverProtocol, DriverRegistry, DriverStartResult, ImageContent,
    PromptAcceptance, PromptContent, ResolvedCapabilities, ResumeResult,
};
pub use driver::{
    ClaudeNativeDriver, CodexNativeDriver, GenericNativeDriver, MockNativeDriver,
    OpenCodeNativeDriver,
};
pub use error::{Result, RuntimeError};
pub use event::{AgentEventEmitter, EventDeduplicator};
pub use instance::{AgentInstance, InstanceManager, InstanceStatus};
pub use provider::{AgentProvider, ProviderContext, ProviderRegistry};
pub use runtime::AgentRuntime;

pub use acp::adapter::AcpProvider;
pub use claude::ClaudeProvider;
pub use codex::CodexProvider;
pub use generic::GenericProvider;
pub use mock::MockProvider;
pub use opencode::OpenCodeProvider;

pub use fleet_core_protocol::rpc::SessionDetail;

/// Boxed stream of unified agent events.
pub type AgentEventStream = futures::stream::BoxStream<'static, Result<AgentEvent>>;

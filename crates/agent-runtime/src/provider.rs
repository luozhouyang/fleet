//! Provider abstraction and registry.

use std::collections::HashMap;
use std::sync::Arc;

use agent_fleet_protocol::agent::{
    AgentCapabilities, AgentSession, AgentStartConfig, PermissionDecision, ProviderInfo,
};
use async_trait::async_trait;

use crate::error::Result;
use crate::event::AgentEventEmitter;

/// Everything a provider needs to start a session.
///
/// The runtime owns the session record and passes a snapshot plus the emitter.
/// Providers must report provider-native identifiers through
/// `agent.raw_event` payloads (`provider_session_id`), which the runtime stores
/// in session metadata.
pub struct ProviderContext {
    /// Machine that owns the session.
    pub machine_id: String,
    /// Session snapshot at start time (status `Starting`).
    pub session: AgentSession,
    /// Per-session event emitter.
    pub emitter: AgentEventEmitter,
}

/// Unified interface implemented by every agent provider adapter.
#[async_trait]
pub trait AgentProvider: Send + Sync {
    /// Stable provider id, for example `claude`, `codex`, `mock`, `generic`.
    fn provider_id(&self) -> &'static str;

    /// Provider descriptor advertised to the control plane.
    fn info(&self) -> ProviderInfo;

    /// Capabilities this adapter genuinely implements.
    fn capabilities(&self) -> AgentCapabilities;

    /// Concrete agent type recorded on the session (defaults to the provider id).
    fn agent_type(&self) -> &'static str {
        self.provider_id()
    }

    /// Spawn or prepare the agent and return the resulting session.
    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession>;

    /// Deliver one user message to a running session.
    async fn send_message(&self, session_id: &str, content: String) -> Result<()>;

    /// Interrupt the current turn.
    async fn interrupt(&self, session_id: &str) -> Result<()>;

    /// Resume an interrupted or restarted session.
    async fn resume(&self, session_id: &str) -> Result<()>;

    /// Terminate the agent process and release provider state.
    async fn shutdown(&self, session_id: &str) -> Result<()>;

    /// Answer a native permission request.
    ///
    /// Providers that route permissions through
    /// [`AgentEventEmitter::request_permission`] are resolved by the runtime
    /// and may implement this as a no-op. Providers with a native approval
    /// channel (for example a JSON-RPC server request) should implement it.
    async fn respond_permission(
        &self,
        session_id: &str,
        decision: PermissionDecision,
    ) -> Result<()>;
}

/// Registry of available provider adapters, keyed by provider id.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn AgentProvider>>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider, replacing any existing provider with the same id.
    pub fn register(&mut self, provider: Arc<dyn AgentProvider>) {
        self.providers
            .insert(provider.provider_id().to_string(), provider);
    }

    /// Look up a provider by id.
    pub fn get(&self, id: &str) -> Option<Arc<dyn AgentProvider>> {
        self.providers.get(id).cloned()
    }

    /// Descriptors for every registered provider, sorted by id.
    pub fn infos(&self) -> Vec<ProviderInfo> {
        let mut infos: Vec<ProviderInfo> = self.providers.values().map(|p| p.info()).collect();
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    /// The preferred default provider: `mock` when present, otherwise the
    /// lexicographically first registered provider.
    pub fn default_provider(&self) -> Option<Arc<dyn AgentProvider>> {
        if let Some(provider) = self.providers.get("mock") {
            return Some(Arc::clone(provider));
        }
        self.providers
            .iter()
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, provider)| Arc::clone(provider))
    }
}

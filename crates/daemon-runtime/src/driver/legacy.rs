//! Compatibility adapter from the old [`AgentProvider`] trait to [`AgentDriver`].
//!
//! This lets the runtime move to driver-based dispatch before every vendor
//! adapter is rewritten. The wrapper is removed once all native adapters
//! implement [`AgentDriver`] directly (see Milestone 9 of the refactor plan).

use std::sync::Arc;

use async_trait::async_trait;
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentSession, AgentStartConfig, PermissionDecision, ProviderInfo,
    ProviderKind, QuestionDecision,
};
use serde_json::{json, Value};

use crate::agent::{AgentDefinition, AgentLaunchConfig};
use crate::error::Result;
use crate::provider::{AgentProvider, ProviderContext};

use super::capabilities::ResolvedCapabilities;
use super::context::DriverContext;
use super::instance::{AgentInstanceHandle, DriverStartResult};
use super::trait_def::{
    AgentDriver, AgentPrompt, DriverInfo, DriverProbeResult, DriverProtocol, PromptAcceptance,
    ResumeResult,
};

/// Wraps one [`AgentProvider`] as an [`AgentDriver`].
pub struct LegacyProviderDriver {
    provider: Arc<dyn AgentProvider>,
    driver_id: String,
}

impl LegacyProviderDriver {
    /// Wrap `provider`, using its provider id as the driver id.
    pub fn new(provider: Arc<dyn AgentProvider>) -> Self {
        Self {
            driver_id: provider.provider_id().to_string(),
            provider,
        }
    }

    /// Wrap `provider` under an explicit driver id.
    ///
    /// Native drivers use ids such as `claude-native` while the legacy
    /// provider id remains `claude`.
    pub fn with_id(provider: Arc<dyn AgentProvider>, driver_id: impl Into<String>) -> Self {
        Self {
            provider,
            driver_id: driver_id.into(),
        }
    }

    /// The wrapped provider.
    pub fn provider(&self) -> &Arc<dyn AgentProvider> {
        &self.provider
    }

    fn provider_protocol(&self) -> DriverProtocol {
        match self.provider.info().kind {
            ProviderKind::Acp => DriverProtocol::Acp,
            _ => DriverProtocol::Native,
        }
    }
}

#[async_trait]
impl AgentDriver for LegacyProviderDriver {
    fn driver_id(&self) -> &str {
        &self.driver_id
    }

    fn info(&self) -> DriverInfo {
        let ProviderInfo {
            id,
            display_name,
            version,
            ..
        } = self.provider.info();
        DriverInfo {
            id: if self.driver_id.is_empty() {
                id
            } else {
                self.driver_id.clone()
            },
            display_name,
            protocol: self.provider_protocol(),
            version,
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        self.provider.capabilities()
    }

    fn provider_info(&self) -> Option<ProviderInfo> {
        Some(self.provider.info())
    }

    async fn probe(&self, definition: &AgentDefinition) -> Result<DriverProbeResult> {
        let info = self.provider.info();
        let capabilities = self.provider.capabilities();
        let mut diagnostics = Vec::new();
        if !info.available {
            diagnostics.push(format!(
                "provider `{}` reports that its executable is unavailable",
                self.provider.provider_id()
            ));
        }
        Ok(DriverProbeResult {
            agent_id: Some(definition.id.clone()),
            driver_id: self.driver_id.clone(),
            protocol: self.provider_protocol(),
            protocol_version: None,
            available: info.available,
            capabilities: ResolvedCapabilities::new(
                capabilities.clone(),
                capabilities.clone(),
                capabilities,
            ),
            agent_info: Some(json!({
                "provider_id": info.id,
                "kind": info.kind,
                "display_name": info.display_name,
                "version": info.version,
            })),
            diagnostics,
        })
    }

    async fn start(
        &self,
        ctx: DriverContext,
        definition: &AgentDefinition,
        config: AgentStartConfig,
    ) -> Result<DriverStartResult> {
        validate_launch(self.provider.as_ref(), definition)?;
        let session_id = ctx.session.id.clone();
        let provider_ctx = ProviderContext {
            machine_id: ctx.machine_id.clone(),
            session: ctx.session.clone(),
            emitter: ctx.emitter.clone(),
        };
        let mut session = self.provider.start(provider_ctx, config).await?;
        if !session.metadata.is_object() {
            session.metadata = json!({});
        }
        let process_id = session
            .metadata
            .get("process_id")
            .or_else(|| session.metadata.get("pid"))
            .and_then(Value::as_u64)
            .map(|pid| pid as u32);
        let metadata = json!({
            "agent": {
                "definition_id": definition.id,
                "driver_id": self.driver_id,
                "instance_id": ctx.instance_id,
            },
            "protocol": {
                "name": self.provider_protocol().as_str(),
            },
            "native": {
                "session_id": session
                    .metadata
                    .get("provider_session_id")
                    .cloned()
                    .unwrap_or(Value::Null),
            },
            "legacy_provider": true,
            "fleet_session_id": session_id,
        });
        Ok(DriverStartResult {
            session,
            process_id,
            metadata,
        })
    }

    async fn send_message(
        &self,
        instance: &AgentInstanceHandle,
        message: AgentPrompt,
    ) -> Result<PromptAcceptance> {
        self.provider
            .send_message(&instance.session_id, message.text_content())
            .await?;
        Ok(PromptAcceptance::now(None))
    }

    async fn interrupt(&self, instance: &AgentInstanceHandle) -> Result<()> {
        self.provider.interrupt(&instance.session_id).await
    }

    async fn resume(
        &self,
        instance: &AgentInstanceHandle,
        session: &AgentSession,
    ) -> Result<ResumeResult> {
        self.provider.resume(&instance.session_id).await?;
        Ok(ResumeResult {
            resumed: true,
            native_session_id: session
                .metadata
                .get("provider_session_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            protocol_version: None,
        })
    }

    async fn shutdown(&self, instance: &AgentInstanceHandle) -> Result<()> {
        self.provider.shutdown(&instance.session_id).await
    }

    async fn respond_permission(
        &self,
        instance: &AgentInstanceHandle,
        decision: PermissionDecision,
    ) -> Result<()> {
        self.provider
            .respond_permission(&instance.session_id, decision)
            .await
    }

    async fn respond_question(
        &self,
        instance: &AgentInstanceHandle,
        decision: QuestionDecision,
    ) -> Result<()> {
        self.provider
            .respond_question(&instance.session_id, decision)
            .await
    }
}

/// Reject definition launch fields that a legacy provider cannot honor.
///
/// Native adapters build their own command line (`bin app-server ...`), so
/// ignoring `launch.args` or a different `launch.executable` would silently
/// start a different program than the definition and probe advertise. The
/// runtime applies `env` and `cwd_policy` before calling the driver, so those
/// fields are supported.
pub(crate) fn validate_launch(
    provider: &dyn AgentProvider,
    definition: &AgentDefinition,
) -> Result<()> {
    let AgentLaunchConfig::Process(process) = &definition.launch else {
        return Ok(());
    };
    if !process.args.is_empty() && process.args != provider.args() {
        return Err(crate::error::RuntimeError::DefinitionInvalid(format!(
            "agent `{}`: the `{}` native driver owns its command-line arguments; \
             remove `launch.args` or use an ACP definition instead",
            definition.id,
            provider.provider_id()
        )));
    }
    if let Some(executable) = provider.executable() {
        if !process.executable.is_empty() && process.executable != executable {
            return Err(crate::error::RuntimeError::DefinitionInvalid(format!(
                "agent `{}`: the `{}` native driver launches `{executable}`, but the \
                 definition configures `{}`; change the provider configuration instead",
                definition.id,
                provider.provider_id(),
                process.executable
            )));
        }
    }
    let defaults = crate::agent::ProcessLaunchConfig::new("");
    if process.startup_timeout_ms != defaults.startup_timeout_ms
        || process.shutdown_timeout_ms != defaults.shutdown_timeout_ms
    {
        return Err(crate::error::RuntimeError::DefinitionInvalid(format!(
            "agent `{}`: the `{}` native driver owns its process timeouts; remove \
             `launch.startup_timeout_ms` / `launch.shutdown_timeout_ms` or use an ACP \
             definition instead",
            definition.id,
            provider.provider_id()
        )));
    }
    Ok(())
}

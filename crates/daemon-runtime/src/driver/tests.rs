//! Unit tests for the driver abstraction.

use std::sync::Arc;

use async_trait::async_trait;
use fleet_core_protocol::agent::{
    AgentCapabilities, AgentSession, AgentStartConfig, PermissionDecision, ProviderInfo,
    ProviderKind, QuestionDecision,
};

use crate::agent::definition::{AgentDefinition, ProcessLaunchConfig};
use crate::error::Result;
use crate::provider::AgentProvider;

use super::capabilities::ResolvedCapabilities;
use super::context::DriverContext;
use super::instance::{AgentInstanceHandle, DriverStartResult};
use super::legacy::{validate_launch, LegacyProviderDriver};
use super::registry::DriverRegistry;
use super::selection::resolve_driver;
use super::trait_def::{
    AgentDriver, AgentPrompt, DriverInfo, DriverProbeResult, DriverProtocol, PromptAcceptance,
    ResumeResult,
};

/// A driver with no protocol behavior, used to test registry and selection.
pub struct StubDriver {
    id: String,
    capabilities: AgentCapabilities,
    available: bool,
}

impl StubDriver {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            capabilities: AgentCapabilities::basic(),
            available: true,
        }
    }
}

#[async_trait]
impl AgentDriver for StubDriver {
    fn driver_id(&self) -> &str {
        &self.id
    }

    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: self.id.clone(),
            display_name: format!("stub {}", self.id),
            protocol: DriverProtocol::Native,
            version: None,
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        self.capabilities.clone()
    }

    async fn probe(&self, definition: &AgentDefinition) -> Result<DriverProbeResult> {
        Ok(DriverProbeResult {
            agent_id: Some(definition.id.clone()),
            driver_id: self.id.clone(),
            protocol: DriverProtocol::Native,
            protocol_version: None,
            available: self.available && definition.enabled,
            capabilities: ResolvedCapabilities::new(
                definition.declared_capabilities.clone(),
                self.capabilities.clone(),
                self.capabilities.clone(),
            ),
            agent_info: None,
            diagnostics: vec![],
        })
    }

    async fn start(
        &self,
        _ctx: DriverContext,
        _definition: &AgentDefinition,
        _config: AgentStartConfig,
    ) -> Result<DriverStartResult> {
        unimplemented!("stub driver does not start sessions")
    }

    async fn send_message(
        &self,
        _instance: &AgentInstanceHandle,
        _message: AgentPrompt,
    ) -> Result<PromptAcceptance> {
        Ok(PromptAcceptance::now(None))
    }

    async fn interrupt(&self, _instance: &AgentInstanceHandle) -> Result<()> {
        Ok(())
    }

    async fn resume(
        &self,
        _instance: &AgentInstanceHandle,
        _session: &AgentSession,
    ) -> Result<ResumeResult> {
        Ok(ResumeResult {
            resumed: true,
            native_session_id: None,
            protocol_version: None,
        })
    }

    async fn shutdown(&self, _instance: &AgentInstanceHandle) -> Result<()> {
        Ok(())
    }

    async fn respond_permission(
        &self,
        _instance: &AgentInstanceHandle,
        _decision: PermissionDecision,
    ) -> Result<()> {
        Ok(())
    }

    async fn respond_question(
        &self,
        _instance: &AgentInstanceHandle,
        _decision: QuestionDecision,
    ) -> Result<()> {
        Ok(())
    }
}

/// A provider whose only observable behavior is its descriptor.
struct StubProvider {
    id: &'static str,
    available: bool,
    executable: Option<&'static str>,
}

#[async_trait]
impl AgentProvider for StubProvider {
    fn provider_id(&self) -> &'static str {
        self.id
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.id.to_string(),
            kind: ProviderKind::Mock,
            display_name: format!("stub {}", self.id),
            version: None,
            available: self.available,
            capabilities: self.capabilities(),
            models: vec![],
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities::basic()
    }

    fn executable(&self) -> Option<&str> {
        self.executable
    }

    async fn start(
        &self,
        _ctx: crate::provider::ProviderContext,
        _config: AgentStartConfig,
    ) -> Result<AgentSession> {
        unimplemented!()
    }

    async fn send_message(&self, _session_id: &str, _content: String) -> Result<()> {
        Ok(())
    }

    async fn interrupt(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn resume(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn respond_permission(
        &self,
        _session_id: &str,
        _decision: PermissionDecision,
    ) -> Result<()> {
        Ok(())
    }

    async fn respond_question(&self, _session_id: &str, _decision: QuestionDecision) -> Result<()> {
        Ok(())
    }
}

fn definition(id: &str, driver_id: &str) -> AgentDefinition {
    let mut definition = AgentDefinition::process(
        id,
        id,
        driver_id,
        ProcessLaunchConfig::new("definitely-not-a-real-binary"),
    );
    definition.declared_capabilities = AgentCapabilities::basic();
    definition
}

#[test]
fn selection_uses_primary_driver() {
    let mut registry = DriverRegistry::new();
    registry.register(Arc::new(StubDriver::new("acp"))).unwrap();
    let selected = resolve_driver(&definition("agent", "acp"), &registry).unwrap();
    assert_eq!(selected.driver_id(), "acp");
}

#[test]
fn selection_uses_fallback_only_when_primary_is_missing() {
    let mut registry = DriverRegistry::new();
    registry
        .register(Arc::new(StubDriver::new("native")))
        .unwrap();
    let mut def = definition("agent", "missing");
    def.fallback_driver = Some("native".into());
    let selected = resolve_driver(&def, &registry).unwrap();
    assert_eq!(selected.driver_id(), "native");

    registry
        .register(Arc::new(StubDriver::new("missing")))
        .unwrap();
    let selected = resolve_driver(&def, &registry).unwrap();
    assert_eq!(selected.driver_id(), "missing");
}

#[test]
fn selection_rejects_disabled_agents_and_unknown_drivers() {
    let registry = DriverRegistry::new();
    let mut def = definition("agent", "acp");
    assert!(matches!(
        resolve_driver(&def, &registry),
        Err(crate::error::RuntimeError::DriverNotFound { .. })
    ));
    def.enabled = false;
    assert!(matches!(
        resolve_driver(&def, &registry),
        Err(crate::error::RuntimeError::AgentDisabled(_))
    ));
}

#[tokio::test]
async fn legacy_driver_reports_provider_descriptor() {
    let provider = Arc::new(StubProvider {
        id: "legacy",
        available: true,
        executable: None,
    });
    let driver = LegacyProviderDriver::new(provider);
    assert_eq!(driver.driver_id(), "legacy");
    assert_eq!(driver.info().display_name, "stub legacy");
    let probe = driver.probe(&definition("agent", "legacy")).await.unwrap();
    assert!(probe.available);
    assert_eq!(probe.driver_id, "legacy");
}

#[tokio::test]
async fn legacy_driver_reports_unavailable_provider() {
    let provider = Arc::new(StubProvider {
        id: "legacy",
        available: false,
        executable: None,
    });
    let driver = LegacyProviderDriver::new(provider);
    let probe = driver.probe(&definition("agent", "legacy")).await.unwrap();
    assert!(!probe.available);
    assert!(!probe.diagnostics.is_empty());
}

#[test]
fn native_driver_rejects_launch_fields_it_cannot_honor() {
    use crate::agent::AgentLaunchConfig;

    let provider = StubProvider {
        id: "claude",
        available: true,
        executable: Some("/opt/claude"),
    };
    let mut definition = definition("agent", "claude-native");
    assert!(matches!(
        validate_launch(&provider, &definition),
        Err(crate::error::RuntimeError::DefinitionInvalid(_))
    ));
    let AgentLaunchConfig::Process(process) = &mut definition.launch else {
        panic!("expected a process launch");
    };
    process.executable = "/opt/claude".to_string();
    process.args = vec!["--verbose".to_string()];
    assert!(matches!(
        validate_launch(&provider, &definition),
        Err(crate::error::RuntimeError::DefinitionInvalid(_))
    ));
    if let AgentLaunchConfig::Process(process) = &mut definition.launch {
        process.args.clear();
        process.startup_timeout_ms += 1;
    }
    assert!(matches!(
        validate_launch(&provider, &definition),
        Err(crate::error::RuntimeError::DefinitionInvalid(_))
    ));
    if let AgentLaunchConfig::Process(process) = &mut definition.launch {
        process.startup_timeout_ms = crate::agent::ProcessLaunchConfig::new("").startup_timeout_ms;
    }
    assert!(validate_launch(&provider, &definition).is_ok());
}

#[test]
fn prompt_renders_images_for_text_only_drivers() {
    let prompt = AgentPrompt {
        content: vec![
            super::trait_def::PromptContent::Text("hello".into()),
            super::trait_def::PromptContent::Image(super::trait_def::ImageContent {
                data: "AAAA".into(),
                mime_type: "image/png".into(),
                name: Some("shot.png".into()),
            }),
        ],
        metadata: serde_json::Value::Null,
    };
    let text = prompt.text_content();
    assert!(text.starts_with("hello"));
    assert!(text.contains("[attachment: shot.png (image/png)]"));
    assert!(text.ends_with("AAAA"));
    assert_eq!(prompt.plain_text(), "hello");
}

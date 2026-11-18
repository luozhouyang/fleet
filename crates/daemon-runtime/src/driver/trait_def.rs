//! The [`AgentDriver`] trait and its value types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use fleet_core_protocol::agent::{
    AgentSession, AgentStartConfig, PermissionDecision, QuestionDecision,
};

use crate::agent::AgentDefinition;
use crate::error::Result;

use super::capabilities::ResolvedCapabilities;
use super::context::DriverContext;
use super::instance::{AgentInstanceHandle, DriverStartResult};

/// Broad protocol family implemented by a driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverProtocol {
    /// Agent Client Protocol.
    Acp,
    /// A protocol defined by the agent vendor.
    Native,
}

impl DriverProtocol {
    /// Stable lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acp => "acp",
            Self::Native => "native",
        }
    }
}

/// Driver descriptor used for diagnostics and the driver catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverInfo {
    /// Stable driver id.
    pub id: String,
    /// Human-readable driver name.
    pub display_name: String,
    /// Protocol family.
    pub protocol: DriverProtocol,
    /// Driver implementation version, when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// One piece of a prompt sent to an agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PromptContent {
    /// Plain text.
    Text(String),
    /// An image attachment.
    Image(ImageContent),
}

/// An image supplied to an agent that accepts images.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageContent {
    /// Base64-encoded image bytes.
    pub data: String,
    /// MIME type, for example `image/png`.
    pub mime_type: String,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A structured prompt.
///
/// Drivers that only accept text use [`AgentPrompt::text_content`], which
/// appends attachments as labelled base64 sections so information is never
/// silently dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPrompt {
    /// Ordered prompt content.
    pub content: Vec<PromptContent>,
    /// Free-form metadata (for example client message ids).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl AgentPrompt {
    /// Build a text-only prompt.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![PromptContent::Text(text.into())],
            metadata: Value::Null,
        }
    }

    /// Whether the prompt has no content.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// The prompt rendered for a text-only driver.
    ///
    /// Image parts are appended as labelled base64 sections. Drivers that
    /// support images should consume [`Self::content`] directly instead.
    pub fn text_content(&self) -> String {
        let mut output = String::new();
        for part in &self.content {
            match part {
                PromptContent::Text(text) => {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(text);
                }
                PromptContent::Image(image) => {
                    if !output.is_empty() {
                        output.push_str("\n\n");
                    }
                    output.push_str("[attachment: ");
                    output.push_str(image.name.as_deref().unwrap_or("image"));
                    output.push_str(" (");
                    output.push_str(&image.mime_type);
                    output.push_str(")]\n");
                    output.push_str(&image.data);
                }
            }
        }
        output
    }

    /// Concatenated text parts, ignoring images.
    pub fn plain_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|part| match part {
                PromptContent::Text(text) => Some(text.as_str()),
                PromptContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Outcome of delivering a prompt.
///
/// This is *not* turn completion. An acceptance only means the agent
/// received the message; the turn finishes when the unified event stream
/// reports a terminal turn event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PromptAcceptance {
    /// The agent acknowledged the prompt.
    Accepted {
        /// Native message id, when the protocol reports one (ACP v2 does).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_message_id: Option<String>,
        /// When the agent accepted the prompt.
        accepted_at: DateTime<Utc>,
    },
    /// The prompt was sent but its acceptance could not be confirmed, for
    /// example because the ACP v2 response timed out.
    ///
    /// The prompt is deliberately not resent: it may already have been
    /// accepted and resending would execute the turn twice.
    AcceptanceUnknown {
        /// Why acceptance could not be confirmed.
        reason: String,
        /// When the uncertainty was recorded.
        accepted_at: DateTime<Utc>,
    },
}

impl PromptAcceptance {
    /// Build an acceptance record stamped with the current time.
    pub fn now(native_message_id: Option<String>) -> Self {
        Self::Accepted {
            native_message_id,
            accepted_at: Utc::now(),
        }
    }

    /// Record an unconfirmed delivery.
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self::AcceptanceUnknown {
            reason: reason.into(),
            accepted_at: Utc::now(),
        }
    }

    /// Native message id reported by an accepted prompt.
    pub fn native_message_id(&self) -> Option<&str> {
        match self {
            Self::Accepted {
                native_message_id, ..
            } => native_message_id.as_deref(),
            Self::AcceptanceUnknown { .. } => None,
        }
    }

    /// Whether the agent confirmed the prompt.
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted { .. })
    }

    /// When the acceptance or uncertainty was recorded.
    pub fn accepted_at(&self) -> DateTime<Utc> {
        match self {
            Self::Accepted { accepted_at, .. } | Self::AcceptanceUnknown { accepted_at, .. } => {
                *accepted_at
            }
        }
    }
}

/// Result of attempting to resume a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeResult {
    /// Whether the agent accepted the resume.
    pub resumed: bool,
    /// Native session id after resume, when changed or newly known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
    /// Negotiated protocol version, when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u16>,
}

/// Result of a lightweight driver probe.
///
/// A probe must not create a persistent session or execute user work. For
/// process drivers it validates the executable and definition; the real
/// handshake happens in [`AgentDriver::start`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverProbeResult {
    /// Agent definition that was probed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Driver that answered the probe.
    pub driver_id: String,
    /// Protocol family.
    pub protocol: DriverProtocol,
    /// Negotiated protocol version when the probe performed a handshake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u16>,
    /// Whether the agent can be launched.
    pub available: bool,
    /// Declared / implemented / verified capability intersection.
    pub capabilities: ResolvedCapabilities,
    /// Agent-reported implementation info, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<Value>,
    /// Human-readable diagnostics (missing executable, unsupported protocol).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

/// The boundary between Fleet and an agent protocol.
#[async_trait]
pub trait AgentDriver: Send + Sync {
    /// Stable id, for example `acp`, `claude-native`.
    fn driver_id(&self) -> &str;

    /// Descriptor for diagnostics and catalogs.
    fn info(&self) -> DriverInfo;

    /// Capabilities this driver genuinely implements for every definition.
    fn capabilities(&self) -> fleet_core_protocol::agent::AgentCapabilities;

    /// Provider descriptor from a wrapped legacy adapter, when this driver has
    /// one.
    ///
    /// Native driver wrappers use this so the provider list keeps reporting the
    /// adapter's own availability, version and models after the runtime moved
    /// to driver-based dispatch. Protocol drivers return `None` and their
    /// definitions are projected from the catalog instead.
    fn provider_info(&self) -> Option<fleet_core_protocol::agent::ProviderInfo> {
        None
    }

    /// Capabilities for one running instance.
    ///
    /// Protocol drivers override this with capabilities negotiated with the
    /// agent (for example image prompt support). The default assumes the
    /// driver-wide capabilities apply.
    fn instance_capabilities(
        &self,
        _instance: &AgentInstanceHandle,
    ) -> fleet_core_protocol::agent::AgentCapabilities {
        self.capabilities()
    }

    /// Validate that this driver can launch `definition` without doing so.
    ///
    /// This is the explicit probe API: protocol drivers may perform a real
    /// handshake here. The runtime uses [`Self::probe_launch`] before starting
    /// a session so it does not spawn the agent twice.
    async fn probe(&self, definition: &AgentDefinition) -> Result<DriverProbeResult>;

    /// Cheap pre-start check (executable, policy) without spawning the agent.
    ///
    /// Defaults to [`Self::probe`] for drivers whose probe is already static.
    async fn probe_launch(&self, definition: &AgentDefinition) -> Result<DriverProbeResult> {
        self.probe(definition).await
    }

    /// Start an agent instance for a Fleet session.
    async fn start(
        &self,
        ctx: DriverContext,
        definition: &AgentDefinition,
        config: AgentStartConfig,
    ) -> Result<DriverStartResult>;

    /// Deliver a prompt.
    ///
    /// A successful return is a [`PromptAcceptance`]: either a confirmed
    /// acceptance or an unconfirmed delivery that the runtime must watch.
    /// Neither means the turn completed; terminal state comes from the
    /// unified event stream.
    async fn send_message(
        &self,
        instance: &AgentInstanceHandle,
        message: AgentPrompt,
    ) -> Result<PromptAcceptance>;

    /// Interrupt the current foreground work.
    async fn interrupt(&self, instance: &AgentInstanceHandle) -> Result<()>;

    /// Resume a previously interrupted session, when the protocol supports it.
    async fn resume(
        &self,
        instance: &AgentInstanceHandle,
        session: &AgentSession,
    ) -> Result<ResumeResult>;

    /// Terminate the agent and release protocol resources.
    async fn shutdown(&self, instance: &AgentInstanceHandle) -> Result<()>;

    /// Answer a permission request over the driver's native channel.
    ///
    /// Drivers that route permissions through
    /// [`crate::event::AgentEventEmitter::request_permission`] may implement
    /// this as a no-op.
    async fn respond_permission(
        &self,
        instance: &AgentInstanceHandle,
        decision: PermissionDecision,
    ) -> Result<()>;

    /// Answer a user question over the driver's native channel.
    async fn respond_question(
        &self,
        instance: &AgentInstanceHandle,
        decision: QuestionDecision,
    ) -> Result<()>;
}

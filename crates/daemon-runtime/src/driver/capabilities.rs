//! Fleet capability model with explicit tri-state status.
//!
//! A boolean is not enough to describe an agent: the agent may declare a
//! capability, the driver may implement it, and a probe may verify it. The
//! effective capabilities exposed to the UI are the intersection of all three.

use fleet_core_protocol::agent::AgentCapabilities;
use serde::{Deserialize, Serialize};

/// Three-state capability status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// Known to work (declared and implemented, and verified when a probe ran).
    Supported,
    /// Known not to work.
    Unsupported,
    /// Not yet determined.
    #[default]
    Unknown,
}

impl CapabilityStatus {
    /// Whether the capability may be offered to a user.
    pub fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

/// Detailed capability model for agents, drivers and probes.
///
/// This is intentionally separate from [`AgentCapabilities`] so that adding
/// fields never breaks the wire format of existing session records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DetailedCapabilities {
    /// Streaming assistant output.
    pub streaming: CapabilityStatus,
    /// Interrupting an active turn.
    pub interrupt: CapabilityStatus,
    /// Resuming an interrupted session.
    pub resume: CapabilityStatus,
    /// Permission approval requests.
    pub permission_approval: CapabilityStatus,
    /// User questions / elicitation.
    pub user_questions: CapabilityStatus,
    /// Structured message/tool events.
    pub structured_events: CapabilityStatus,
    /// Structured file diffs.
    pub structured_diff: CapabilityStatus,
    /// Agent-side session persistence.
    pub session_persistence: CapabilityStatus,
    /// Tool call events.
    pub tool_calls: CapabilityStatus,
    /// Image prompt input.
    pub images: CapabilityStatus,
    /// First-class plan updates.
    pub plans: CapabilityStatus,
    /// Token usage reporting.
    pub usage_reporting: CapabilityStatus,
    /// Agent-owned terminal output display.
    pub terminal_output: CapabilityStatus,
    /// Available command catalog.
    pub available_commands: CapabilityStatus,
    /// Session configuration options.
    pub config_options: CapabilityStatus,
}

impl DetailedCapabilities {
    /// Convert the legacy boolean model into detailed statuses.
    pub fn from_agent_capabilities(capabilities: &AgentCapabilities) -> Self {
        fn status(value: bool) -> CapabilityStatus {
            if value {
                CapabilityStatus::Supported
            } else {
                CapabilityStatus::Unsupported
            }
        }
        Self {
            streaming: status(capabilities.streaming),
            interrupt: status(capabilities.interrupt),
            resume: status(capabilities.resume),
            permission_approval: status(capabilities.permission_approval),
            user_questions: status(capabilities.user_questions),
            structured_events: status(capabilities.structured_events),
            structured_diff: status(capabilities.structured_diff),
            session_persistence: status(capabilities.session_persistence),
            tool_calls: status(capabilities.tool_calls),
            images: status(capabilities.images),
            plans: CapabilityStatus::Unknown,
            usage_reporting: CapabilityStatus::Unknown,
            terminal_output: CapabilityStatus::Unknown,
            available_commands: CapabilityStatus::Unknown,
            config_options: CapabilityStatus::Unknown,
        }
    }

    /// Collapse to the legacy boolean model.
    ///
    /// Only [`CapabilityStatus::Supported`] becomes `true`; `Unknown` is
    /// reported as unsupported so the UI never promises an unverified
    /// capability.
    pub fn to_agent_capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: self.streaming.is_supported(),
            interrupt: self.interrupt.is_supported(),
            resume: self.resume.is_supported(),
            permission_approval: self.permission_approval.is_supported(),
            user_questions: self.user_questions.is_supported(),
            structured_events: self.structured_events.is_supported(),
            structured_diff: self.structured_diff.is_supported(),
            session_persistence: self.session_persistence.is_supported(),
            tool_calls: self.tool_calls.is_supported(),
            images: self.images.is_supported(),
        }
    }

    /// Downgrade every `Supported` status to `Unsupported` unless the other
    /// side also supports it.
    pub fn intersect(&self, other: &Self) -> Self {
        fn both(a: CapabilityStatus, b: CapabilityStatus) -> CapabilityStatus {
            match (a, b) {
                (CapabilityStatus::Supported, CapabilityStatus::Supported) => {
                    CapabilityStatus::Supported
                }
                (CapabilityStatus::Unsupported, _) | (_, CapabilityStatus::Unsupported) => {
                    CapabilityStatus::Unsupported
                }
                _ => CapabilityStatus::Unknown,
            }
        }
        Self {
            streaming: both(self.streaming, other.streaming),
            interrupt: both(self.interrupt, other.interrupt),
            resume: both(self.resume, other.resume),
            permission_approval: both(self.permission_approval, other.permission_approval),
            user_questions: both(self.user_questions, other.user_questions),
            structured_events: both(self.structured_events, other.structured_events),
            structured_diff: both(self.structured_diff, other.structured_diff),
            session_persistence: both(self.session_persistence, other.session_persistence),
            tool_calls: both(self.tool_calls, other.tool_calls),
            images: both(self.images, other.images),
            plans: both(self.plans, other.plans),
            usage_reporting: both(self.usage_reporting, other.usage_reporting),
            terminal_output: both(self.terminal_output, other.terminal_output),
            available_commands: both(self.available_commands, other.available_commands),
            config_options: both(self.config_options, other.config_options),
        }
    }
}

/// Capabilities at the three levels the platform must distinguish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCapabilities {
    /// What the agent definition says the agent supports.
    pub declared: AgentCapabilities,
    /// What the driver can implement.
    pub driver_supported: AgentCapabilities,
    /// What a probe actually verified.
    pub verified: AgentCapabilities,
}

impl ResolvedCapabilities {
    /// Build a resolution from the three boolean capability sets.
    pub fn new(
        declared: AgentCapabilities,
        driver_supported: AgentCapabilities,
        verified: AgentCapabilities,
    ) -> Self {
        Self {
            declared,
            driver_supported,
            verified,
        }
    }

    /// Capabilities the UI may offer.
    ///
    /// Never exceeds the driver's supported capabilities, and never claims
    /// something the probe did not verify.
    pub fn effective(&self) -> AgentCapabilities {
        intersect_agent_capabilities(
            &intersect_agent_capabilities(&self.declared, &self.driver_supported),
            &self.verified,
        )
    }

    /// The same resolution in the detailed tri-state model.
    pub fn detailed(&self) -> DetailedCapabilities {
        DetailedCapabilities::from_agent_capabilities(&self.effective())
    }
}

fn intersect_agent_capabilities(a: &AgentCapabilities, b: &AgentCapabilities) -> AgentCapabilities {
    AgentCapabilities {
        streaming: a.streaming && b.streaming,
        interrupt: a.interrupt && b.interrupt,
        resume: a.resume && b.resume,
        permission_approval: a.permission_approval && b.permission_approval,
        user_questions: a.user_questions && b.user_questions,
        structured_events: a.structured_events && b.structured_events,
        structured_diff: a.structured_diff && b.structured_diff,
        session_persistence: a.session_persistence && b.session_persistence,
        tool_calls: a.tool_calls && b.tool_calls,
        images: a.images && b.images,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_never_exceeds_driver_capabilities() {
        let declared = AgentCapabilities::full();
        let mut driver = AgentCapabilities::basic();
        driver.tool_calls = true;
        let mut verified = AgentCapabilities::full();
        verified.images = false;
        let resolved = ResolvedCapabilities::new(declared, driver, verified);
        let effective = resolved.effective();
        assert!(effective.streaming);
        assert!(effective.tool_calls);
        assert!(!effective.resume);
        assert!(!effective.images);
        assert!(!effective.permission_approval);
    }

    #[test]
    fn detailed_status_roundtrip() {
        let capabilities = AgentCapabilities::full();
        let detailed = DetailedCapabilities::from_agent_capabilities(&capabilities);
        assert_eq!(detailed.to_agent_capabilities(), capabilities);
        let unknown = DetailedCapabilities::default();
        assert!(!unknown.to_agent_capabilities().streaming);
    }
}

use serde::{Deserialize, Serialize};

/// Broad provider family. The wire value is stable and snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Claude,
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    Generic,
    Mock,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Generic => "generic",
            Self::Mock => "mock",
        }
    }

    pub fn all() -> &'static [ProviderKind] {
        &[
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::OpenCode,
            ProviderKind::Generic,
            ProviderKind::Mock,
        ]
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Capabilities a provider adapter genuinely supports.
///
/// Adapters must declare only what they actually implement; they must not
/// emulate missing capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub interrupt: bool,
    pub resume: bool,
    pub permission_approval: bool,
    pub structured_events: bool,
    pub structured_diff: bool,
    pub session_persistence: bool,
    pub tool_calls: bool,
    pub images: bool,
}

impl AgentCapabilities {
    /// Everything this platform can render today.
    pub fn full() -> Self {
        Self {
            streaming: true,
            interrupt: true,
            resume: true,
            permission_approval: true,
            structured_events: true,
            structured_diff: true,
            session_persistence: true,
            tool_calls: true,
            images: true,
        }
    }

    /// Minimal structured provider (for example a JSONL CLI without approvals).
    pub fn basic() -> Self {
        Self {
            streaming: true,
            structured_events: true,
            ..Self::default()
        }
    }
}

/// Provider descriptor advertised by a machine to the control plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub kind: ProviderKind,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub available: bool,
    pub capabilities: AgentCapabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&ProviderKind::OpenCode).unwrap(), "\"opencode\"");
        assert_eq!(ProviderKind::Mock.as_str(), "mock");
    }

    #[test]
    fn provider_info_roundtrip() {
        let info = ProviderInfo {
            id: "claude".into(),
            kind: ProviderKind::Claude,
            display_name: "Claude Code".into(),
            version: Some("2.1.319".into()),
            available: true,
            capabilities: AgentCapabilities::full(),
            models: vec!["default".into()],
        };
        let back: ProviderInfo =
            serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
        assert_eq!(info, back);
    }
}

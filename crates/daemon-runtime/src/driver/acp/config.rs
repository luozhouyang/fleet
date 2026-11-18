//! Generic ACP driver configuration and protocol policy.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Which ACP protocol versions the driver may negotiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AcpProtocolPolicy {
    /// Try v2 first when it is enabled, then fall back to v1. This is the
    /// default.
    #[default]
    Auto,
    /// Only negotiate ACP v1.
    V1Only,
    /// Only negotiate ACP v2. Requires the `acp_v2` feature flag.
    V2Only,
}

impl AcpProtocolPolicy {
    /// Parse a configuration value.
    pub fn parse_policy(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "v1" | "v1_only" | "v1only" => Some(Self::V1Only),
            "v2" | "v2_only" | "v2only" => Some(Self::V2Only),
            _ => None,
        }
    }
}

/// A protocol version chosen for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpProtocolVersion {
    /// ACP protocol v1.
    V1,
    /// ACP protocol v2 (draft).
    V2,
}

impl AcpProtocolVersion {
    /// Numeric wire version.
    pub fn number(self) -> u16 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
}

impl std::fmt::Display for AcpProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.number())
    }
}

/// Configuration shared by every ACP agent definition.
#[derive(Debug, Clone)]
pub struct AcpDriverConfig {
    /// Client name advertised during `initialize`.
    pub client_name: String,
    /// Client version advertised during `initialize`.
    pub client_version: String,
    /// Protocol negotiation policy.
    pub protocol_policy: AcpProtocolPolicy,
    /// Feature flag: when `false`, v2 is never negotiated even if the agent
    /// supports it.
    pub acp_v2_enabled: bool,
    /// Bound for spawning the process and completing `initialize`.
    pub startup_timeout: Duration,
    /// Bound for graceful shutdown before the process is killed.
    pub shutdown_timeout: Duration,
    /// How long a permission request may stay unanswered.
    pub permission_timeout: Duration,
    /// How long a question may stay unanswered.
    pub question_timeout: Duration,
}

impl Default for AcpDriverConfig {
    fn default() -> Self {
        Self {
            client_name: "fleet-daemon".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_policy: AcpProtocolPolicy::Auto,
            acp_v2_enabled: false,
            startup_timeout: Duration::from_secs(15),
            shutdown_timeout: Duration::from_secs(5),
            permission_timeout: Duration::from_secs(600),
            question_timeout: Duration::from_secs(600),
        }
    }
}

impl AcpDriverConfig {
    /// Build the configuration from daemon settings.
    pub fn new(
        client_name: impl Into<String>,
        protocol_policy: AcpProtocolPolicy,
        acp_v2_enabled: bool,
    ) -> Self {
        Self {
            client_name: client_name.into(),
            protocol_policy,
            acp_v2_enabled,
            ..Self::default()
        }
    }

    /// Protocols to attempt, in order.
    ///
    /// Returns an empty list when `V2Only` is configured but the v2 feature
    /// flag is off; callers must surface that as a configuration error rather
    /// than silently using v1.
    pub fn protocol_attempts(&self) -> Vec<AcpProtocolVersion> {
        match self.protocol_policy {
            AcpProtocolPolicy::V1Only => vec![AcpProtocolVersion::V1],
            AcpProtocolPolicy::V2Only => {
                if self.acp_v2_enabled {
                    vec![AcpProtocolVersion::V2]
                } else {
                    Vec::new()
                }
            }
            AcpProtocolPolicy::Auto => {
                if self.acp_v2_enabled {
                    vec![AcpProtocolVersion::V2, AcpProtocolVersion::V1]
                } else {
                    vec![AcpProtocolVersion::V1]
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_tries_v2_first_only_when_enabled() {
        let mut config = AcpDriverConfig::default();
        assert_eq!(config.protocol_attempts(), vec![AcpProtocolVersion::V1]);
        config.acp_v2_enabled = true;
        assert_eq!(
            config.protocol_attempts(),
            vec![AcpProtocolVersion::V2, AcpProtocolVersion::V1]
        );
    }

    #[test]
    fn v2_only_with_disabled_flag_is_empty() {
        let config = AcpDriverConfig {
            protocol_policy: AcpProtocolPolicy::V2Only,
            acp_v2_enabled: false,
            ..AcpDriverConfig::default()
        };
        assert!(config.protocol_attempts().is_empty());
    }

    #[test]
    fn policy_parsing_and_version_numbers() {
        assert_eq!(
            AcpProtocolPolicy::parse_policy("V1").unwrap(),
            AcpProtocolPolicy::V1Only
        );
        assert_eq!(
            AcpProtocolPolicy::parse_policy("auto").unwrap(),
            AcpProtocolPolicy::Auto
        );
        assert!(AcpProtocolPolicy::parse_policy("v3").is_none());
        assert_eq!(AcpProtocolVersion::V1.number(), 1);
        assert_eq!(AcpProtocolVersion::V2.to_string(), "v2");
    }
}

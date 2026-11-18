//! The agent definition model.

use std::collections::BTreeMap;

use fleet_core_protocol::agent::AgentCapabilities;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where an agent definition came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentDefinitionSource {
    /// Compiled into the daemon.
    Builtin,
    /// Loaded from the user's daemon configuration.
    UserConfig,
    /// Loaded from a workspace configuration (reserved; not implemented yet).
    WorkspaceConfig,
    /// Fetched from a remote registry (reserved; not implemented yet).
    RemoteRegistry,
}

impl AgentDefinitionSource {
    /// Stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::UserConfig => "user_config",
            Self::WorkspaceConfig => "workspace_config",
            Self::RemoteRegistry => "remote_registry",
        }
    }
}

/// Who chooses the working directory for an agent process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CwdPolicy {
    /// Use the working directory supplied with the session request.
    #[default]
    Session,
    /// Always use a fixed directory.
    Fixed(String),
    /// Use the registered workspace root of the session.
    WorkspaceRoot,
}

/// Launch configuration for a locally spawned agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessLaunchConfig {
    /// Executable name or absolute path.
    pub executable: String,
    /// Command-line arguments. Arguments are passed as argv entries, never
    /// through a shell.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables. Values may reference the daemon
    /// environment through `${VAR}` indirection at load time.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working-directory policy.
    #[serde(default)]
    pub cwd_policy: CwdPolicy,
    /// How long starting the agent may take.
    #[serde(default = "default_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
    /// How long a graceful shutdown may take before the process is killed.
    #[serde(default = "default_shutdown_timeout_ms")]
    pub shutdown_timeout_ms: u64,
}

fn default_startup_timeout_ms() -> u64 {
    10_000
}

fn default_shutdown_timeout_ms() -> u64 {
    5_000
}

impl ProcessLaunchConfig {
    /// Create a launch configuration for `executable`.
    pub fn new(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd_policy: CwdPolicy::Session,
            startup_timeout_ms: default_startup_timeout_ms(),
            shutdown_timeout_ms: default_shutdown_timeout_ms(),
        }
    }
}

/// Launch configuration for an agent reachable over the network.
///
/// Remote agents are reserved for a later version; definitions using this
/// variant fail validation until a remote transport is implemented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLaunchConfig {
    /// Transport endpoint (for example a WebSocket URL).
    pub endpoint: String,
    /// Optional authentication token reference. Tokens themselves must not be
    /// stored in the definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_ref: Option<String>,
}

/// How the daemon launches an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AgentLaunchConfig {
    /// A child process owned by this daemon.
    Process(ProcessLaunchConfig),
    /// A remote agent endpoint.
    Remote(RemoteLaunchConfig),
}

impl AgentLaunchConfig {
    /// The process launch configuration, when this agent is local.
    pub fn process(&self) -> Option<&ProcessLaunchConfig> {
        match self {
            Self::Process(config) => Some(config),
            Self::Remote(_) => None,
        }
    }
}

/// A launchable agent.
///
/// The definition is a snapshot-able value: sessions persist a copy so that
/// editing the registry later cannot change the meaning of an existing
/// session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Stable, globally unique agent id, for example `claude-code`.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Optional description shown by the agent catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Driver used to speak with this agent.
    pub driver_id: String,
    /// How to launch the agent.
    pub launch: AgentLaunchConfig,
    /// Driver to use when [`Self::driver_id`] is unavailable. Fallback is only
    /// allowed before a session is created (see
    /// [`crate::driver::selection`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_driver: Option<String>,
    /// Fallback driver used only when the primary driver cannot be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_driver: Option<String>,
    /// Capabilities the agent itself declares. The driver validates these
    /// against what it can actually implement and probe.
    #[serde(default)]
    pub declared_capabilities: AgentCapabilities,
    /// Disabled definitions cannot create new sessions. Running sessions are
    /// never killed by disabling a definition.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Where this definition came from.
    #[serde(default = "default_source")]
    pub source: AgentDefinitionSource,
    /// Free-form extension data (icon, vendor, model hints, provider kind).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

fn default_enabled() -> bool {
    true
}

fn default_source() -> AgentDefinitionSource {
    AgentDefinitionSource::Builtin
}

impl AgentDefinition {
    /// Build a minimal process-launched definition.
    pub fn process(
        id: impl Into<String>,
        name: impl Into<String>,
        driver_id: impl Into<String>,
        launch: ProcessLaunchConfig,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            driver_id: driver_id.into(),
            launch: AgentLaunchConfig::Process(launch),
            preferred_driver: None,
            fallback_driver: None,
            declared_capabilities: AgentCapabilities::default(),
            enabled: true,
            source: AgentDefinitionSource::Builtin,
            metadata: Value::Null,
        }
    }

    /// Validate the definition before it is registered.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("agent definition id must not be empty".to_string());
        }
        if self.driver_id.trim().is_empty() {
            return Err(format!("agent definition `{}` must name a driver", self.id));
        }
        match &self.launch {
            AgentLaunchConfig::Process(process) => {
                if process.executable.trim().is_empty() {
                    return Err(format!(
                        "agent definition `{}` must name an executable",
                        self.id
                    ));
                }
                if process.startup_timeout_ms == 0 {
                    return Err(format!(
                        "agent definition `{}` must have a non-zero startup timeout",
                        self.id
                    ));
                }
            }
            AgentLaunchConfig::Remote(_) => {
                return Err(format!(
                    "agent definition `{}` uses remote launch, which is not implemented yet",
                    self.id
                ));
            }
        }
        Ok(())
    }

    /// A JSON snapshot safe to persist on a session.
    ///
    /// Environment variables are deliberately omitted: they can contain
    /// credentials and are never written to the transcript database.
    pub fn snapshot(&self) -> Value {
        let launch = match &self.launch {
            AgentLaunchConfig::Process(process) => serde_json::json!({
                "type": "process",
                "executable": process.executable,
                "args": process.args,
                "cwd_policy": process.cwd_policy,
                "startup_timeout_ms": process.startup_timeout_ms,
                "shutdown_timeout_ms": process.shutdown_timeout_ms,
            }),
            AgentLaunchConfig::Remote(remote) => serde_json::json!({
                "type": "remote",
                "endpoint": remote.endpoint,
            }),
        };
        serde_json::json!({
            "agent_definition_id": self.id,
            "driver_id": self.driver_id,
            "definition_version": 1,
            "launch_snapshot": launch,
        })
    }

    /// Resolve the working directory for a session.
    ///
    /// `session_cwd` is the directory chosen by the daemon for the session;
    /// `workspace_root` is the registered workspace root, when one exists.
    pub fn resolve_cwd(&self, session_cwd: &str, workspace_root: Option<&str>) -> Option<String> {
        match &self.launch {
            AgentLaunchConfig::Process(process) => match &process.cwd_policy {
                CwdPolicy::Session => Some(session_cwd.to_string()),
                CwdPolicy::Fixed(path) => Some(path.clone()),
                CwdPolicy::WorkspaceRoot => workspace_root
                    .map(str::to_string)
                    .or_else(|| Some(session_cwd.to_string())),
            },
            AgentLaunchConfig::Remote(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_omits_environment_and_includes_launch() {
        let mut launch = ProcessLaunchConfig::new("claude");
        launch.args = vec!["--foo".into()];
        launch.env.insert("SECRET".into(), "value".into());
        let definition = AgentDefinition::process("claude-code", "Claude", "claude-native", launch);
        let snapshot = definition.snapshot();
        assert_eq!(snapshot["agent_definition_id"], "claude-code");
        assert_eq!(snapshot["launch_snapshot"]["executable"], "claude");
        assert!(snapshot["launch_snapshot"].get("env").is_none());
        assert!(!snapshot.to_string().contains("SECRET"));
    }

    #[test]
    fn validation_rejects_empty_ids_and_remote_launch() {
        let mut definition =
            AgentDefinition::process("", "x", "acp", ProcessLaunchConfig::new("x"));
        assert!(definition.validate().is_err());
        definition.id = "ok".into();
        definition.launch = AgentLaunchConfig::Remote(RemoteLaunchConfig {
            endpoint: "ws://example".into(),
            token_ref: None,
        });
        assert!(definition.validate().is_err());
    }

    #[test]
    fn cwd_policy_resolution() {
        let mut launch = ProcessLaunchConfig::new("x");
        let mut definition = AgentDefinition::process("a", "a", "acp", launch.clone());
        assert_eq!(
            definition.resolve_cwd("/session", Some("/workspace")),
            Some("/session".to_string())
        );
        launch.cwd_policy = CwdPolicy::Fixed("/fixed".into());
        definition.launch = AgentLaunchConfig::Process(launch.clone());
        assert_eq!(
            definition.resolve_cwd("/session", Some("/workspace")),
            Some("/fixed".to_string())
        );
        launch.cwd_policy = CwdPolicy::WorkspaceRoot;
        definition.launch = AgentLaunchConfig::Process(launch);
        assert_eq!(
            definition.resolve_cwd("/session", Some("/workspace")),
            Some("/workspace".to_string())
        );
    }
}

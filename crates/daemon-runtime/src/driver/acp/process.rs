//! Process configuration for an ACP agent.
//!
//! The ACP SDK's [`AcpAgent`](agent_client_protocol::AcpAgent) owns the child
//! process, its stdio pipes, stderr capture and process-group teardown. This
//! module validates the definition and builds the SDK configuration; Fleet
//! must not add a second process manager with different teardown semantics.

use std::collections::BTreeMap;

use agent_client_protocol::AcpAgentConfig;

use crate::agent::{AgentDefinition, AgentLaunchConfig, ProcessLaunchConfig};
use crate::error::{Result, RuntimeError};

/// The validated process configuration for one ACP connection.
pub struct AcpProcessConfig {
    /// SDK launch configuration.
    pub agent: AcpAgentConfig,
    /// Human-readable executable, used in diagnostics.
    pub executable: String,
    /// Non-sensitive argument snapshot.
    pub args: Vec<String>,
}

/// Build the process configuration for `definition`.
///
/// Session-level environment variables override definition environment
/// variables. Secrets are never persisted; this structure is only used to
/// spawn the child.
pub fn process_config(
    definition: &AgentDefinition,
    session_env: &BTreeMap<String, String>,
) -> Result<AcpProcessConfig> {
    let AgentLaunchConfig::Process(process) = &definition.launch else {
        return Err(RuntimeError::DefinitionInvalid(format!(
            "agent `{}` does not use a process launch configuration",
            definition.id
        )));
    };
    validate_process(process, &definition.id)?;

    let mut env = BTreeMap::new();
    env.extend(process.env.clone());
    env.extend(session_env.clone());

    let agent = AcpAgentConfig::new(&process.executable)
        .args(process.args.clone())
        .envs(env);
    Ok(AcpProcessConfig {
        agent,
        executable: process.executable.clone(),
        args: process.args.clone(),
    })
}

fn validate_process(process: &ProcessLaunchConfig, agent_id: &str) -> Result<()> {
    if process.executable.trim().is_empty() {
        return Err(RuntimeError::DefinitionInvalid(format!(
            "agent `{agent_id}` has no executable"
        )));
    }
    if process.executable.contains('\0') {
        return Err(RuntimeError::DefinitionInvalid(format!(
            "agent `{agent_id}` executable contains a NUL byte"
        )));
    }
    for arg in &process.args {
        if arg.contains('\0') {
            return Err(RuntimeError::DefinitionInvalid(format!(
                "agent `{agent_id}` has an argument containing a NUL byte"
            )));
        }
    }
    for (key, value) in &process.env {
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            return Err(RuntimeError::DefinitionInvalid(format!(
                "agent `{agent_id}` has an invalid environment variable name `{key}`"
            )));
        }
        if value.contains('\0') {
            return Err(RuntimeError::DefinitionInvalid(format!(
                "agent `{agent_id}` environment variable `{key}` contains a NUL byte"
            )));
        }
    }
    if !crate::provider::binary_exists(&process.executable) {
        return Err(RuntimeError::ProviderUnavailable(format!(
            "ACP executable `{}` was not found",
            process.executable
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentDefinition;

    #[test]
    fn rejects_missing_executables_and_empty_arguments() {
        let definition = AgentDefinition::process(
            "a",
            "a",
            "acp",
            ProcessLaunchConfig::new("definitely-not-a-real-binary"),
        );
        assert!(matches!(
            process_config(&definition, &BTreeMap::new()),
            Err(RuntimeError::ProviderUnavailable(_))
        ));

        let mut launch = ProcessLaunchConfig::new("sh");
        launch.args = vec!["ok".into(), "bad\0arg".into()];
        let definition = AgentDefinition::process("a", "a", "acp", launch);
        assert!(matches!(
            process_config(&definition, &BTreeMap::new()),
            Err(RuntimeError::DefinitionInvalid(_))
        ));
    }

    #[test]
    fn session_environment_overrides_definition_environment() {
        let mut launch = ProcessLaunchConfig::new("sh");
        launch.env.insert("A".into(), "definition".into());
        launch.env.insert("B".into(), "definition".into());
        let definition = AgentDefinition::process("a", "a", "acp", launch);
        let mut session = BTreeMap::new();
        session.insert("B".into(), "session".into());
        let config = process_config(&definition, &session).unwrap();
        assert_eq!(config.agent.environment().get("A").unwrap(), "definition");
        assert_eq!(config.agent.environment().get("B").unwrap(), "session");
    }
}

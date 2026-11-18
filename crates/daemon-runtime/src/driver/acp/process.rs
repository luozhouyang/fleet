//! Process configuration for an ACP agent.
//!
//! By default the ACP SDK's [`AcpAgent`](agent_client_protocol::AcpAgent)
//! owns the child process, its stdio pipes, stderr capture and process-group
//! teardown. This module validates the definition and builds the SDK
//! configuration so there is exactly one process manager on that path.
//!
//! Definitions that set `inherit_env` opt into an explicit environment
//! policy. The SDK does not expose `Command::env_clear`, so those definitions
//! use [`FleetAgentSpawner`] instead: it spawns the same child with a cleared
//! environment, its own process group and the same stdio protocol transport,
//! and hands the stdio streams to the SDK protocol layer. Teardown matches
//! the SDK's: stdin closes first, then the process group is killed after the
//! configured shutdown timeout.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, ByteStreams, Client, ConnectTo, Error as AcpError, ErrorCode,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::agent::{AgentDefinition, AgentLaunchConfig, ProcessLaunchConfig};
use crate::error::{Result, RuntimeError};

/// How the ACP child process is launched.
#[derive(Clone)]
pub enum AcpAgentLauncher {
    /// The SDK spawns the process; the child inherits the daemon environment.
    Inherit(AcpAgentConfig),
    /// Fleet spawns the process from an explicitly constructed environment.
    Isolated(FleetAgentSpawner),
}

impl AcpAgentLauncher {
    /// Environment overrides configured for the child, when any.
    pub fn environment(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Inherit(config) => config.environment(),
            Self::Isolated(spawner) => &spawner.env,
        }
    }

    /// Whether the child starts from a cleared environment.
    pub fn is_isolated(&self) -> bool {
        matches!(self, Self::Isolated(_))
    }
}

impl ConnectTo<Client> for AcpAgentLauncher {
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> std::result::Result<(), AcpError> {
        match self {
            Self::Inherit(config) => {
                ConnectTo::<Client>::connect_to(AcpAgent::new(config), client).await
            }
            Self::Isolated(spawner) => spawner.connect_to(client).await,
        }
    }
}

/// Spawns an ACP agent with a cleared environment.
///
/// Only the configured variables are visible to the child. The process runs
/// in its own process group, its stderr is drained for diagnostics, and it is
/// killed with its whole group when the connection future is dropped.
#[derive(Clone)]
pub struct FleetAgentSpawner {
    executable: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    shutdown_timeout: Duration,
}

impl ConnectTo<Client> for FleetAgentSpawner {
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> std::result::Result<(), AcpError> {
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(&self.args)
            .env_clear()
            .envs(&self.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().map_err(|error| {
            AcpError::new(
                ErrorCode::INTERNAL_ERROR,
                format!("failed to start `{}`: {error}", self.executable),
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AcpError::new(
                ErrorCode::INTERNAL_ERROR,
                "agent stdin pipe is unavailable".to_string(),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AcpError::new(
                ErrorCode::INTERNAL_ERROR,
                "agent stdout pipe is unavailable".to_string(),
            )
        })?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let line = line.trim_end();
                            if !line.is_empty() {
                                tracing::debug!(agent_stderr = line, "acp agent stderr");
                            }
                        }
                    }
                }
            });
        }

        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        let protocol = ConnectTo::<Client>::connect_to(transport, client);
        tokio::pin!(protocol);
        let mut guard = ProcessGuard { child };
        tokio::select! {
            result = &mut protocol => {
                // The protocol ended. stdin was dropped with the transport, so
                // give the agent a bounded window to exit before killing it.
                let _ = tokio::time::timeout(self.shutdown_timeout, guard.wait()).await;
                guard.terminate();
                result
            }
            status = guard.wait() => {
                // The agent exited first; give the transport a moment to
                // observe EOF and drain buffered frames.
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut protocol).await;
                match status {
                    Ok(_) => Ok(()),
                    Err(error) => Err(AcpError::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("failed to wait for `{}`: {error}", self.executable),
                    )),
                }
            }
        }
    }
}

/// Owns the child and terminates its process group on drop.
struct ProcessGuard {
    child: tokio::process::Child,
}

impl ProcessGuard {
    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // `process_group(0)` made the child a group leader, so the
            // negative pid targets wrappers and their descendants.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// The validated process configuration for one ACP connection.
pub struct AcpProcessConfig {
    /// Launcher used to start the child and speak ACP over its stdio.
    pub launcher: AcpAgentLauncher,
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

    let launcher = match &process.inherit_env {
        None => {
            let mut env = BTreeMap::new();
            env.extend(process.env.clone());
            env.extend(session_env.clone());
            AcpAgentLauncher::Inherit(
                AcpAgentConfig::new(&process.executable)
                    .args(process.args.clone())
                    .envs(env),
            )
        }
        Some(inherit) => {
            let parent: BTreeMap<String, String> = std::env::vars().collect();
            AcpAgentLauncher::Isolated(FleetAgentSpawner {
                executable: process.executable.clone(),
                args: process.args.clone(),
                env: resolve_child_environment(inherit, &process.env, session_env, &parent),
                shutdown_timeout: Duration::from_millis(process.shutdown_timeout_ms.max(1)),
            })
        }
    };
    Ok(AcpProcessConfig {
        launcher,
        executable: process.executable.clone(),
        args: process.args.clone(),
    })
}

/// Build the child environment for an explicit inheritance policy.
///
/// Only variables named in `inherit` are copied from the parent. Definition
/// and session values are applied afterwards so they can override inherited
/// ones.
fn resolve_child_environment(
    inherit: &[String],
    definition: &BTreeMap<String, String>,
    session: &BTreeMap<String, String>,
    parent: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for name in inherit {
        if let Some(value) = parent.get(name) {
            env.insert(name.clone(), value.clone());
        }
    }
    env.extend(definition.clone());
    env.extend(session.clone());
    env
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
    if let Some(inherit_env) = &process.inherit_env {
        for key in inherit_env {
            if key.is_empty() || key.contains('=') || key.contains('\0') {
                return Err(RuntimeError::DefinitionInvalid(format!(
                    "agent `{agent_id}` has an invalid inherited environment variable name `{key}`"
                )));
            }
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
        assert_eq!(
            config.launcher.environment().get("A").unwrap(),
            "definition"
        );
        assert_eq!(config.launcher.environment().get("B").unwrap(), "session");
        assert!(!config.launcher.is_isolated());
    }

    #[test]
    fn inherit_env_defaults_to_full_inheritance() {
        let mut launch = ProcessLaunchConfig::new("sh");
        launch.env.insert("A".into(), "definition".into());
        let definition = AgentDefinition::process("a", "a", "acp", launch);
        let config = process_config(&definition, &BTreeMap::new()).unwrap();
        assert!(!config.launcher.is_isolated());
        assert_eq!(
            config.launcher.environment().get("A").unwrap(),
            "definition"
        );
    }

    #[test]
    fn inherit_env_selects_only_the_listed_parent_variables() {
        let parent: BTreeMap<String, String> = [
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("SECRET".to_string(), "do-not-leak".to_string()),
            ("HOME".to_string(), "/home/agent".to_string()),
        ]
        .into_iter()
        .collect();
        let definition_env = [("FROM_DEFINITION".to_string(), "yes".to_string())]
            .into_iter()
            .collect();
        let mut session = BTreeMap::new();
        session.insert("PATH".to_string(), "/session/bin".to_string());

        let env = resolve_child_environment(
            &[
                "PATH".to_string(),
                "HOME".to_string(),
                "MISSING".to_string(),
            ],
            &definition_env,
            &session,
            &parent,
        );
        assert_eq!(env.get("PATH").unwrap(), "/session/bin");
        assert_eq!(env.get("HOME").unwrap(), "/home/agent");
        assert_eq!(env.get("FROM_DEFINITION").unwrap(), "yes");
        assert!(!env.contains_key("SECRET"));
        assert!(!env.contains_key("MISSING"));
    }

    #[test]
    fn inherit_env_uses_the_isolated_launcher() {
        let mut launch = ProcessLaunchConfig::new("sh");
        launch.inherit_env = Some(Vec::new());
        let definition = AgentDefinition::process("a", "a", "acp", launch);
        let config = process_config(&definition, &BTreeMap::new()).unwrap();
        assert!(config.launcher.is_isolated());
        assert!(config.launcher.environment().is_empty());
    }

    #[test]
    fn inherit_env_rejects_invalid_names() {
        let mut launch = ProcessLaunchConfig::new("sh");
        launch.inherit_env = Some(vec!["BAD=NAME".to_string()]);
        let definition = AgentDefinition::process("a", "a", "acp", launch);
        assert!(matches!(
            process_config(&definition, &BTreeMap::new()),
            Err(RuntimeError::DefinitionInvalid(_))
        ));
    }
}

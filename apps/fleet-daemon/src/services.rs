use std::sync::Arc;

use fleet_core_auth::MachineIdentity;
use fleet_core_config::DaemonConfig;
use fleet_core_protocol::agent::ProviderInfo;
use fleet_daemon_git::GitService;
use fleet_daemon_runtime::{
    AgentRuntime, ClaudeProvider, CodexProvider, GenericProvider, MockProvider, OpenCodeProvider,
    ProviderRegistry,
};
use fleet_daemon_storage::DaemonStorage;
use fleet_daemon_workspace::{WorkspaceService, WorkspaceSettings};

/// Long-lived services owned by the daemon.
pub struct Services {
    pub config: DaemonConfig,
    pub workspaces: WorkspaceService,
    pub git: GitService,
    pub runtime: AgentRuntime,
    /// Daemon database, including the durable event outbox.
    pub storage: Arc<DaemonStorage>,
    pub default_provider: String,
    pub machine_id: String,
    pub identity: MachineIdentity,
    /// Previous key kept only until the agent admin accepts a rotation.
    pub previous_identity: Option<MachineIdentity>,
    pub started_at: std::time::Instant,
}

impl Services {
    /// Build long-running services and restore interrupted sessions.
    pub async fn build(config: DaemonConfig) -> anyhow::Result<Self> {
        Self::build_inner(config, true).await
    }

    /// Build services for a short-lived CLI command.
    ///
    /// This must not mutate live session state: running `register-workspace` or
    /// `providers` while the daemon is up must not mark sessions interrupted or
    /// cancel permissions.
    pub async fn build_local(config: DaemonConfig) -> anyhow::Result<Self> {
        Self::build_inner(config, false).await
    }

    async fn build_inner(config: DaemonConfig, restore: bool) -> anyhow::Result<Self> {
        let mut config = config;
        config.resolve_paths();

        if let Some(parent) = config.storage.database.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = config.machine.key_file.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let storage = Arc::new(DaemonStorage::connect(&config.storage.database).await?);
        let workspaces = WorkspaceService::new(
            storage.clone(),
            WorkspaceSettings {
                max_file_bytes: config.workspace.max_file_bytes,
                default_ignores: config.workspace.default_ignores.clone(),
                allowed_roots: config.workspace.allowed_roots.clone(),
            },
        );
        let git = GitService::new();

        let identity = MachineIdentity::load_or_create(&config.machine.key_file)?;
        let previous_identity = MachineIdentity::load_previous(&config.machine.key_file)?;
        let machine_id = match config.machine.id.clone() {
            Some(id) => id,
            None => load_or_create_machine_id(&config.machine.key_file, &identity)?,
        };

        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(MockProvider::new()));
        registry.register(Arc::new(ClaudeProvider::new(
            config.agent.claude_bin.clone(),
        )));
        registry.register(Arc::new(CodexProvider::new(config.agent.codex_bin.clone())));
        let opencode = OpenCodeProvider::from_env()
            .unwrap_or_else(|| OpenCodeProvider::new(config.agent.opencode_bin.clone()));
        registry.register(Arc::new(opencode));

        if let Ok(bin) = std::env::var("AGENT_FLEET_GENERIC_BIN") {
            if !bin.is_empty() {
                let args = std::env::var("AGENT_FLEET_GENERIC_ARGS")
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                registry.register(Arc::new(GenericProvider::new(bin, args)));
            }
        }

        let default_provider = config.agent.default_provider.clone();
        let runtime = AgentRuntime::new(registry, 4096).with_storage(storage.clone());
        if restore {
            runtime
                .restore_sessions(&machine_id)
                .await
                .map_err(|error| anyhow::anyhow!("failed to restore session history: {error}"))?;
        }

        Ok(Self {
            config,
            workspaces,
            git,
            runtime,
            storage,
            default_provider,
            machine_id,
            identity,
            previous_identity,
            started_at: std::time::Instant::now(),
        })
    }

    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.runtime.providers()
    }
}

/// Stable machine id stored next to the key file.
///
/// The id must survive key rotation, so it is persisted separately from the
/// key's fingerprint instead of being recomputed on every start.
fn load_or_create_machine_id(
    key_file: &std::path::Path,
    identity: &MachineIdentity,
) -> anyhow::Result<String> {
    let mut id_path = key_file.as_os_str().to_os_string();
    id_path.push(".id");
    let id_path = std::path::PathBuf::from(id_path);
    if id_path.exists() {
        let value = std::fs::read_to_string(&id_path)?;
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    let value = identity.fingerprint();
    if let Some(parent) = id_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&id_path, format!("{value}\n"))?;
    Ok(value)
}

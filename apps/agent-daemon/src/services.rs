use std::sync::Arc;

use agent_fleet_agent_runtime::{
    AgentRuntime, ClaudeProvider, CodexProvider, GenericProvider, MockProvider, OpenCodeProvider,
    ProviderRegistry,
};
use agent_fleet_auth::MachineIdentity;
use agent_fleet_config::DaemonConfig;
use agent_fleet_git_service::GitService;
use agent_fleet_protocol::agent::ProviderInfo;
use agent_fleet_storage::Database;
use agent_fleet_workspace::{WorkspaceService, WorkspaceSettings};

/// Long-lived services owned by the daemon.
pub struct Services {
    pub config: DaemonConfig,
    pub workspaces: WorkspaceService,
    pub git: GitService,
    pub runtime: AgentRuntime,
    pub default_provider: String,
    pub machine_id: String,
    pub identity: MachineIdentity,
    pub started_at: std::time::Instant,
}

impl Services {
    pub async fn build(config: DaemonConfig) -> anyhow::Result<Self> {
        let mut config = config;
        config.resolve_paths();

        if let Some(parent) = config.storage.database.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = config.machine.key_file.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let storage = Arc::new(Database::connect(&config.storage.database).await?);
        let workspaces = WorkspaceService::new(
            storage.clone(),
            WorkspaceSettings {
                max_file_bytes: config.workspace.max_file_bytes,
                default_ignores: config.workspace.default_ignores.clone(),
            },
        );
        let git = GitService::new();

        let identity = MachineIdentity::load_or_create(&config.machine.key_file)?;
        let machine_id = config
            .machine
            .id
            .clone()
            .unwrap_or_else(|| identity.fingerprint());

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

        Ok(Self {
            config,
            workspaces,
            git,
            runtime,
            default_provider,
            machine_id,
            identity,
            started_at: std::time::Instant::now(),
        })
    }

    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.runtime.providers()
    }
}

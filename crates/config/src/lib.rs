//! Configuration loading for the Agent Fleet daemon and control plane.
//!
//! Configuration is layered:
//!
//! 1. Built-in defaults.
//! 2. An optional TOML file.
//! 3. `AGENT_FLEET_*` environment variables.
//!
//! The TOML layout mirrors `docs/implementation.md` §5.3:
//!
//! ```toml
//! [machine]
//! name = "My Laptop"
//!
//! [daemon]
//! host = "127.0.0.1"
//! port = 18080
//!
//! [storage]
//! database = "~/.agent-fleet/agent.db"
//! ```
//!
//! Paths are stored as written (including a leading `~`). Call
//! [`DaemonConfig::resolve_paths`] or [`ControlPlaneConfig::resolve_paths`]
//! before using them so that `~` is expanded and parent directories exist.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Size of a mebibyte.
const MIB: u64 = 1024 * 1024;

/// Default machine key path.
const DEFAULT_KEY_FILE: &str = "~/.agent-fleet/machine.key";
/// Default daemon database path.
const DEFAULT_DAEMON_DB: &str = "~/.agent-fleet/agent.db";
/// Default control-plane database path.
const DEFAULT_CONTROL_PLANE_DB: &str = "~/.agent-fleet/control-plane.db";
/// Default ignore list for workspace traversal.
const DEFAULT_IGNORES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    ".next",
    "__pycache__",
    ".venv",
];

// ---------------------------------------------------------------------------
// Daemon configuration
// ---------------------------------------------------------------------------

/// Complete configuration for `agent-daemon`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Machine identity and key storage.
    pub machine: MachineConfig,
    /// Local HTTP server addresses.
    pub daemon: DaemonServerConfig,
    /// Local database location.
    pub storage: StorageConfig,
    /// Agent provider defaults.
    pub agent: AgentConfig,
    /// Workspace traversal defaults.
    pub workspace: WorkspaceConfig,
    /// Control-plane connection settings.
    pub connection: ConnectionConfig,
    /// Git write-operation policy.
    pub git: GitConfig,
}

impl DaemonConfig {
    /// Load configuration from `path` (if it exists), then apply environment
    /// overrides.
    ///
    /// A missing file is not an error; built-in defaults are used instead.
    /// Paths are not expanded by this call; invoke [`Self::resolve_paths`].
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let mut config = match path {
            Some(path) if path.exists() => Self::from_str(&read_config_file(path)?)?,
            _ => Self::default(),
        };
        config.apply_env()?;
        Ok(config)
    }

    /// Parse configuration from TOML without reading the environment.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(toml_src: &str) -> anyhow::Result<Self> {
        let config = toml::from_str(toml_src)
            .map_err(|err| anyhow::anyhow!("failed to parse daemon configuration: {err}"))?;
        Ok(config)
    }

    /// Load the default configuration file.
    ///
    /// Checks `AGENT_FLEET_CONFIG`, then `~/.agent-fleet/config.toml`, then
    /// built-in defaults.
    pub fn load_default() -> anyhow::Result<Self> {
        Self::load(default_config_path().as_deref())
    }

    /// Expand a leading `~` in configured paths and create the parent
    /// directories for the database and machine key.
    pub fn resolve_paths(&mut self) {
        self.machine.key_file = expand_tilde(&self.machine.key_file);
        ensure_parent_dir(&self.machine.key_file);

        self.storage.database = expand_tilde(&self.storage.database);
        ensure_parent_dir(&self.storage.database);

        if let Some(root) = self.workspace.default_root.take() {
            self.workspace.default_root = Some(expand_tilde(&root));
        }
    }

    fn apply_env(&mut self) -> anyhow::Result<()> {
        if let Ok(data_dir) = std::env::var(ENV_DATA_DIR) {
            let data_dir = PathBuf::from(data_dir);
            self.storage.database = data_dir.join("agent.db");
            self.machine.key_file = data_dir.join("machine.key");
        }
        if let Ok(database) = std::env::var(ENV_DB) {
            self.storage.database = PathBuf::from(database);
        }
        if let Ok(name) = std::env::var(ENV_MACHINE_NAME) {
            self.machine.name = Some(name);
        }
        if let Ok(url) = std::env::var(ENV_SERVER_URL) {
            self.connection.server_url = Some(url);
        }
        if let Ok(token) = std::env::var(ENV_TOKEN) {
            self.connection.token = Some(token);
        }
        if let Ok(bind) = std::env::var(ENV_BIND) {
            let (host, port) = parse_bind(&bind)?;
            self.daemon.host = host;
            self.daemon.port = port;
        }
        if let Ok(value) = std::env::var(ENV_GIT_WRITES) {
            self.git.allow_writes = parse_bool(ENV_GIT_WRITES, &value)?;
        }
        Ok(())
    }
}

/// Machine identity settings for the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MachineConfig {
    /// Stable machine id, when already registered.
    pub id: Option<String>,
    /// Human-readable machine name.
    pub name: Option<String>,
    /// Path to the Ed25519 machine identity key file.
    pub key_file: PathBuf,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: None,
            key_file: PathBuf::from(DEFAULT_KEY_FILE),
        }
    }
}

/// Local daemon HTTP listener configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonServerConfig {
    /// Interface to bind the daemon health/metrics server to.
    pub host: String,
    /// Port to bind the daemon health/metrics server to.
    pub port: u16,
}

impl Default for DaemonServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 18080,
        }
    }
}

/// SQLite storage configuration shared by both binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Path to the SQLite database file.
    pub database: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            database: PathBuf::from(DEFAULT_DAEMON_DB),
        }
    }
}

/// Agent provider executable and selection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Provider used when a request does not name one.
    pub default_provider: String,
    /// Claude Code executable.
    pub claude_bin: String,
    /// Codex executable.
    pub codex_bin: String,
    /// OpenCode executable.
    pub opencode_bin: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            default_provider: "mock".to_string(),
            claude_bin: "claude".to_string(),
            codex_bin: "codex".to_string(),
            opencode_bin: "opencode".to_string(),
        }
    }
}

/// Workspace traversal and file-read defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    /// Default root offered when registering a workspace.
    pub default_root: Option<PathBuf>,
    /// Maximum number of bytes returned for a single file.
    pub max_file_bytes: u64,
    /// Directory names excluded from traversal by default.
    pub default_ignores: Vec<String>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            default_root: None,
            max_file_bytes: 2 * MIB,
            default_ignores: DEFAULT_IGNORES.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

/// Control-plane connection settings for the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    /// WebSocket URL of the control plane.
    pub server_url: Option<String>,
    /// Bearer token used to authenticate with the control plane.
    pub token: Option<String>,
    /// Heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,
    /// Maximum reconnect backoff in milliseconds.
    pub reconnect_max_ms: u64,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            server_url: None,
            token: None,
            heartbeat_interval_ms: 15_000,
            reconnect_max_ms: 30_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Control-plane configuration
// ---------------------------------------------------------------------------

/// Complete configuration for `control-plane`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlPlaneConfig {
    /// Public HTTP API listener configuration.
    pub server: ServerConfig,
    /// Control-plane database location.
    pub storage: StorageConfig,
    /// Admin authentication settings.
    pub auth: AuthConfig,
    /// Machine connection policy.
    pub machines: MachinePolicy,
    /// Static web asset settings.
    pub web: WebConfig,
    /// Git write-operation policy.
    pub git: GitConfig,
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            storage: StorageConfig {
                database: PathBuf::from(DEFAULT_CONTROL_PLANE_DB),
            },
            auth: AuthConfig::default(),
            machines: MachinePolicy::default(),
            web: WebConfig::default(),
            git: GitConfig::default(),
        }
    }
}

impl ControlPlaneConfig {
    /// Load configuration from `path` (if it exists), then apply environment
    /// overrides.
    ///
    /// A missing file is not an error; built-in defaults are used instead.
    /// Paths are not expanded by this call; invoke [`Self::resolve_paths`].
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let mut config = match path {
            Some(path) if path.exists() => Self::from_str(&read_config_file(path)?)?,
            _ => Self::default(),
        };
        config.apply_env()?;
        Ok(config)
    }

    /// Parse configuration from TOML without reading the environment.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(toml_src: &str) -> anyhow::Result<Self> {
        let config = toml::from_str(toml_src)
            .map_err(|err| anyhow::anyhow!("failed to parse control-plane configuration: {err}"))?;
        Ok(config)
    }

    /// Load the default configuration file.
    ///
    /// Checks `AGENT_FLEET_CONFIG`, then `~/.agent-fleet/config.toml`, then
    /// built-in defaults.
    pub fn load_default() -> anyhow::Result<Self> {
        Self::load(default_config_path().as_deref())
    }

    /// Expand a leading `~` in configured paths and create the parent
    /// directory for the database.
    pub fn resolve_paths(&mut self) {
        self.storage.database = expand_tilde(&self.storage.database);
        ensure_parent_dir(&self.storage.database);

        if let Some(static_dir) = self.web.static_dir.take() {
            self.web.static_dir = Some(expand_tilde(&static_dir));
        }
    }

    fn apply_env(&mut self) -> anyhow::Result<()> {
        if let Ok(data_dir) = std::env::var(ENV_DATA_DIR) {
            self.storage.database = PathBuf::from(data_dir).join("control-plane.db");
        }
        if let Ok(database) = std::env::var(ENV_DB) {
            self.storage.database = PathBuf::from(database);
        }
        if let Ok(token) = std::env::var(ENV_ADMIN_TOKEN) {
            self.auth.admin_token = token;
        }
        if let Ok(bind) = std::env::var(ENV_BIND) {
            let (host, port) = parse_bind(&bind)?;
            self.server.host = host;
            self.server.port = port;
        }
        if let Ok(static_dir) = std::env::var(ENV_WEB_DIR) {
            self.web.static_dir = Some(PathBuf::from(static_dir));
        }
        if let Ok(value) = std::env::var(ENV_GIT_WRITES) {
            self.git.allow_writes = parse_bool(ENV_GIT_WRITES, &value)?;
        }
        if let Ok(token) = std::env::var(ENV_GIT_WRITE_TOKEN) {
            let token = token.trim();
            self.git.write_token = if token.is_empty() {
                None
            } else {
                Some(token.to_string())
            };
        }
        Ok(())
    }
}

/// External HTTP listener configuration for the control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Interface to bind the HTTP API to.
    pub host: String,
    /// Port to bind the HTTP API to.
    pub port: u16,
    /// Optional externally visible base URL.
    pub public_url: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8787,
            public_url: None,
        }
    }
}

/// Control-plane administrator authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Bearer token accepted by the REST API and machine WebSocket.
    pub admin_token: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            admin_token: "dev-token".to_string(),
        }
    }
}

/// Heartbeat and request timeouts for machine connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MachinePolicy {
    /// Expected heartbeat interval in milliseconds.
    pub heartbeat_interval_ms: u64,
    /// Time without a heartbeat after which a machine is considered offline.
    pub heartbeat_timeout_ms: u64,
    /// Timeout applied to proxied daemon requests.
    pub request_timeout_ms: u64,
}

impl Default for MachinePolicy {
    fn default() -> Self {
        Self {
            heartbeat_interval_ms: 15_000,
            heartbeat_timeout_ms: 45_000,
            request_timeout_ms: 30_000,
        }
    }
}

/// Git write-operation policy.
///
/// Write requests are always authenticated with the regular bearer token; when
/// `write_token` is set they additionally require
/// `X-Agent-Fleet-Git-Token: <token>`. Set `allow_writes = false` to disable
/// stage/commit/checkout entirely. The daemon honours `allow_writes` too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// Whether Git write endpoints are available.
    pub allow_writes: bool,
    /// Optional dedicated token required in `X-Agent-Fleet-Git-Token`.
    pub write_token: Option<String>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            allow_writes: true,
            write_token: None,
        }
    }
}

/// Static web asset configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Directory containing the built web UI, when served by the control plane.
    pub static_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Environment variable names
// ---------------------------------------------------------------------------

/// Environment variable pointing directly at a TOML config file.
pub const ENV_CONFIG: &str = "AGENT_FLEET_CONFIG";
/// Environment variable overriding [`ConnectionConfig::server_url`].
pub const ENV_SERVER_URL: &str = "AGENT_FLEET_SERVER_URL";
/// Environment variable overriding [`ConnectionConfig::token`].
pub const ENV_TOKEN: &str = "AGENT_FLEET_TOKEN";
/// Environment variable overriding the SQLite database path.
pub const ENV_DB: &str = "AGENT_FLEET_DB";
/// Environment variable deriving the default database and key paths.
pub const ENV_DATA_DIR: &str = "AGENT_FLEET_DATA_DIR";
/// Environment variable overriding [`MachineConfig::name`].
pub const ENV_MACHINE_NAME: &str = "AGENT_FLEET_MACHINE_NAME";
/// Environment variable overriding [`AuthConfig::admin_token`].
pub const ENV_ADMIN_TOKEN: &str = "AGENT_FLEET_ADMIN_TOKEN";
/// Environment variable overriding the listener address (`host:port`).
pub const ENV_BIND: &str = "AGENT_FLEET_BIND";
/// Environment variable overriding [`WebConfig::static_dir`].
pub const ENV_WEB_DIR: &str = "AGENT_FLEET_WEB_DIR";
/// Environment variable enabling/disabling Git writes (`1`/`0`, `true`/`false`).
pub const ENV_GIT_WRITES: &str = "AGENT_FLEET_GIT_WRITES";
/// Environment variable setting [`GitConfig::write_token`].
pub const ENV_GIT_WRITE_TOKEN: &str = "AGENT_FLEET_GIT_WRITE_TOKEN";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_config_file(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path)
        .map_err(|err| anyhow::anyhow!("failed to read config file {}: {err}", path.display()))
}

fn default_config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(ENV_CONFIG) {
        return Some(PathBuf::from(path));
    }
    dirs::home_dir().map(|home| home.join(".agent-fleet").join("config.toml"))
}

fn parse_bool(name: &str, value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(anyhow::anyhow!(
            "invalid {name} value `{other}`: expected one of 1, 0, true, false, yes, no, on, off"
        )),
    }
}

fn parse_bind(value: &str) -> anyhow::Result<(String, u16)> {
    let (host, port) = value.rsplit_once(':').ok_or_else(|| {
        anyhow::anyhow!("invalid AGENT_FLEET_BIND value `{value}`: expected host:port")
    })?;
    let port: u16 = port
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid AGENT_FLEET_BIND port `{port}`: expected 0-65535"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err(anyhow::anyhow!(
            "invalid AGENT_FLEET_BIND value `{value}`: empty host"
        ));
    }
    Ok((host.to_string(), port))
}

/// Expand a leading `~` using the current user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };

    let rest = if text == "~" {
        Some("")
    } else {
        text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\"))
    };

    match rest {
        Some(rest) => match dirs::home_dir() {
            Some(home) if rest.is_empty() => home,
            Some(home) => home.join(rest),
            None => path.to_path_buf(),
        },
        None => path.to_path_buf(),
    }
}

/// Create the parent directory of `path`, ignoring empty parents.
fn ensure_parent_dir(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if parent.as_os_str().is_empty() {
        return;
    }
    if let Err(err) = std::fs::create_dir_all(parent) {
        tracing::warn!(
            path = %parent.display(),
            error = %err,
            "failed to create configuration directory"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_KEYS: &[&str] = &[
        ENV_CONFIG,
        ENV_SERVER_URL,
        ENV_TOKEN,
        ENV_DB,
        ENV_DATA_DIR,
        ENV_MACHINE_NAME,
        ENV_ADMIN_TOKEN,
        ENV_BIND,
        ENV_WEB_DIR,
        ENV_GIT_WRITES,
        ENV_GIT_WRITE_TOKEN,
    ];

    /// Serializes tests that mutate process-wide environment variables and
    /// removes those variables again on drop.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for key in ENV_KEYS {
                std::env::remove_var(key);
            }
            Self { _lock: lock }
        }

        fn set(&self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
            std::env::set_var(key, value);
        }

        fn remove(&self, key: &str) {
            std::env::remove_var(key);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for key in ENV_KEYS {
                std::env::remove_var(key);
            }
        }
    }

    #[test]
    fn daemon_defaults_match_contract() {
        let config = DaemonConfig::default();

        assert_eq!(config.machine.id, None);
        assert_eq!(config.machine.name, None);
        assert_eq!(
            config.machine.key_file,
            PathBuf::from("~/.agent-fleet/machine.key")
        );
        assert_eq!(config.daemon.host, "127.0.0.1");
        assert_eq!(config.daemon.port, 18080);
        assert_eq!(
            config.storage.database,
            PathBuf::from("~/.agent-fleet/agent.db")
        );
        assert_eq!(config.agent.default_provider, "mock");
        assert_eq!(config.agent.claude_bin, "claude");
        assert_eq!(config.agent.codex_bin, "codex");
        assert_eq!(config.workspace.default_root, None);
        assert_eq!(config.workspace.max_file_bytes, 2 * 1024 * 1024);
        assert_eq!(
            config.workspace.default_ignores,
            vec![
                ".git",
                "node_modules",
                "target",
                "dist",
                ".next",
                "__pycache__",
                ".venv"
            ]
        );
        assert_eq!(config.connection.server_url, None);
        assert_eq!(config.connection.token, None);
        assert_eq!(config.connection.heartbeat_interval_ms, 15_000);
        assert_eq!(config.connection.reconnect_max_ms, 30_000);
    }

    #[test]
    fn control_plane_defaults_match_contract() {
        let config = ControlPlaneConfig::default();

        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8787);
        assert_eq!(config.server.public_url, None);
        assert_eq!(
            config.storage.database,
            PathBuf::from("~/.agent-fleet/control-plane.db")
        );
        assert_eq!(config.auth.admin_token, "dev-token");
        assert_eq!(config.machines.heartbeat_interval_ms, 15_000);
        assert_eq!(config.machines.heartbeat_timeout_ms, 45_000);
        assert_eq!(config.machines.request_timeout_ms, 30_000);
        assert_eq!(config.web.static_dir, None);
        assert!(config.git.allow_writes);
        assert_eq!(config.git.write_token, None);
    }

    #[test]
    fn parses_daemon_toml_and_ignores_unknown_keys() {
        let src = r#"
unknown_root_key = "ignored"

[machine]
id = "machine-1"
name = "Test Machine"
key_file = "/tmp/custom.key"

[daemon]
host = "0.0.0.0"
port = 19000

[storage]
database = "/tmp/agent.db"

[agent]
default_provider = "claude"
claude_bin = "/opt/claude"
codex_bin = "/opt/codex"

[workspace]
default_root = "/tmp/projects"
max_file_bytes = 4096
default_ignores = ["target", "dist"]

[connection]
server_url = "ws://127.0.0.1:8787/ws/machine"
token = "secret"
heartbeat_interval_ms = 1000
reconnect_max_ms = 2000
"#;

        let config = DaemonConfig::from_str(src).expect("valid TOML");
        assert_eq!(config.machine.id.as_deref(), Some("machine-1"));
        assert_eq!(config.machine.name.as_deref(), Some("Test Machine"));
        assert_eq!(config.machine.key_file, PathBuf::from("/tmp/custom.key"));
        assert_eq!(config.daemon.host, "0.0.0.0");
        assert_eq!(config.daemon.port, 19000);
        assert_eq!(config.storage.database, PathBuf::from("/tmp/agent.db"));
        assert_eq!(config.agent.default_provider, "claude");
        assert_eq!(config.agent.claude_bin, "/opt/claude");
        assert_eq!(config.agent.codex_bin, "/opt/codex");
        assert_eq!(
            config.workspace.default_root,
            Some(PathBuf::from("/tmp/projects"))
        );
        assert_eq!(config.workspace.max_file_bytes, 4096);
        assert_eq!(config.workspace.default_ignores, vec!["target", "dist"]);
        assert_eq!(
            config.connection.server_url.as_deref(),
            Some("ws://127.0.0.1:8787/ws/machine")
        );
        assert_eq!(config.connection.token.as_deref(), Some("secret"));
        assert_eq!(config.connection.heartbeat_interval_ms, 1000);
        assert_eq!(config.connection.reconnect_max_ms, 2000);
    }

    #[test]
    fn parses_control_plane_toml() {
        let src = r#"
[server]
host = "0.0.0.0"
port = 9000
public_url = "https://fleet.example.com"

[storage]
database = "/var/lib/agent-fleet/control-plane.db"

[auth]
admin_token = "admin-secret"

[machines]
heartbeat_interval_ms = 5000
heartbeat_timeout_ms = 20000
request_timeout_ms = 10000

[web]
static_dir = "/srv/agent-fleet/web"
"#;

        let config = ControlPlaneConfig::from_str(src).expect("valid TOML");
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
        assert_eq!(
            config.server.public_url.as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(
            config.storage.database,
            PathBuf::from("/var/lib/agent-fleet/control-plane.db")
        );
        assert_eq!(config.auth.admin_token, "admin-secret");
        assert_eq!(config.machines.heartbeat_interval_ms, 5000);
        assert_eq!(config.machines.heartbeat_timeout_ms, 20000);
        assert_eq!(config.machines.request_timeout_ms, 10000);
        assert_eq!(
            config.web.static_dir,
            Some(PathBuf::from("/srv/agent-fleet/web"))
        );
    }

    #[test]
    fn env_overrides_toml_and_defaults() {
        let guard = EnvGuard::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let data_dir = temp.path().join("data");
        let custom_db = temp.path().join("custom.db");
        let web_dir = temp.path().join("web");

        guard.set(ENV_DATA_DIR, &data_dir);
        guard.set(ENV_SERVER_URL, "ws://example.test:1234/ws/machine");
        guard.set(ENV_TOKEN, "machine-token");
        guard.set(ENV_MACHINE_NAME, "test-machine");
        guard.set(ENV_BIND, "0.0.0.0:19001");
        guard.set(ENV_GIT_WRITES, "false");
        guard.set(ENV_GIT_WRITE_TOKEN, "git-secret");

        let daemon = DaemonConfig::load(None).expect("daemon config");
        assert_eq!(daemon.storage.database, data_dir.join("agent.db"));
        assert_eq!(daemon.machine.key_file, data_dir.join("machine.key"));
        assert_eq!(daemon.machine.name.as_deref(), Some("test-machine"));
        assert_eq!(
            daemon.connection.server_url.as_deref(),
            Some("ws://example.test:1234/ws/machine")
        );
        assert_eq!(daemon.connection.token.as_deref(), Some("machine-token"));
        assert_eq!(daemon.daemon.host, "0.0.0.0");
        assert_eq!(daemon.daemon.port, 19001);
        assert!(!daemon.git.allow_writes);

        // An explicit database path wins over the derived data-dir path.
        guard.set(ENV_DB, &custom_db);
        let daemon = DaemonConfig::load(None).expect("daemon config");
        assert_eq!(daemon.storage.database, custom_db);

        guard.set(ENV_ADMIN_TOKEN, "admin-secret");
        guard.set(ENV_WEB_DIR, &web_dir);
        let control_plane = ControlPlaneConfig::load(None).expect("control-plane config");
        assert_eq!(control_plane.storage.database, custom_db);
        assert_eq!(control_plane.auth.admin_token, "admin-secret");
        assert_eq!(control_plane.server.host, "0.0.0.0");
        assert_eq!(control_plane.server.port, 19001);
        assert_eq!(control_plane.web.static_dir, Some(web_dir));
        assert!(!control_plane.git.allow_writes);
        assert_eq!(control_plane.git.write_token.as_deref(), Some("git-secret"));

        guard.set(ENV_GIT_WRITES, "bogus");
        assert!(ControlPlaneConfig::load(None).is_err());
        guard.set(ENV_GIT_WRITES, "true");

        // Without an explicit database, the data dir supplies the default.
        guard.remove(ENV_DB);
        let control_plane = ControlPlaneConfig::load(None).expect("control-plane config");
        assert_eq!(
            control_plane.storage.database,
            data_dir.join("control-plane.db")
        );
    }

    #[test]
    fn file_values_take_precedence_over_defaults() {
        let guard = EnvGuard::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[daemon]
port = 19999

[storage]
database = "/tmp/from-file.db"
"#,
        )
        .expect("write config");

        let config = DaemonConfig::load(Some(&config_path)).expect("load config");
        assert_eq!(config.daemon.port, 19999);
        assert_eq!(config.daemon.host, "127.0.0.1");
        assert_eq!(config.storage.database, PathBuf::from("/tmp/from-file.db"));

        guard.set(ENV_BIND, "127.0.0.1:17777");
        guard.set(ENV_DB, "/tmp/from-env.db");
        let config = DaemonConfig::load(Some(&config_path)).expect("load config");
        assert_eq!(config.daemon.port, 17777);
        assert_eq!(config.storage.database, PathBuf::from("/tmp/from-env.db"));
    }

    #[test]
    fn resolve_paths_expands_tilde_and_creates_parents() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nested_db = temp.path().join("nested").join("dir").join("agent.db");
        let mut config = DaemonConfig::from_str(&format!(
            "[storage]\ndatabase = \"{}\"\n",
            nested_db.display()
        ))
        .expect("config");
        config.resolve_paths();
        assert_eq!(config.storage.database, nested_db);
        assert!(nested_db.parent().expect("parent").is_dir());

        let Some(home) = dirs::home_dir() else {
            return;
        };
        let dir_name = format!(".agent-fleet-config-test-{}", std::process::id());
        let mut config = DaemonConfig::from_str(&format!(
            "[storage]\ndatabase = \"~/{dir_name}/agent.db\"\n\
             [machine]\nkey_file = \"~/{dir_name}/machine.key\"\n"
        ))
        .expect("config");
        config.resolve_paths();

        assert_eq!(
            config.storage.database,
            home.join(&dir_name).join("agent.db")
        );
        assert_eq!(
            config.machine.key_file,
            home.join(&dir_name).join("machine.key")
        );
        assert!(home.join(&dir_name).is_dir());

        let _ = std::fs::remove_dir_all(home.join(&dir_name));
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(DaemonConfig::from_str("this is not valid toml [[[").is_err());
        assert!(ControlPlaneConfig::from_str("[server\nport =").is_err());

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("bad.toml");
        std::fs::write(&path, "[[[").expect("write");
        assert!(DaemonConfig::load(Some(&path)).is_err());
        assert!(ControlPlaneConfig::load(Some(&path)).is_err());
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let _guard = EnvGuard::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing.toml");
        let config = DaemonConfig::load(Some(&missing)).expect("load config");
        assert_eq!(config.daemon.port, 18080);
    }

    #[test]
    fn example_configs_parse() {
        let daemon: DaemonConfig =
            toml::from_str(include_str!("../examples/daemon.toml")).expect("daemon example");
        assert_eq!(daemon.daemon.port, 18080);

        let control_plane: ControlPlaneConfig =
            toml::from_str(include_str!("../examples/control-plane.toml"))
                .expect("control-plane example");
        assert_eq!(control_plane.server.port, 8787);
    }

    #[test]
    fn parses_ipv6_bind_address() {
        assert_eq!(
            parse_bind("[::1]:8787").expect("bind"),
            ("::1".to_string(), 8787)
        );
        assert!(parse_bind("127.0.0.1").is_err());
        assert!(parse_bind("127.0.0.1:not-a-port").is_err());
    }
}

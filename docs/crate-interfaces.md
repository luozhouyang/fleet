# Crate Interfaces (MVP Implementation Contract)

This document freezes the internal Rust interfaces used by the MVP. Every crate
must expose at least the API described here so that `apps/agent-daemon` and
`apps/control-plane` compose without rework. See `docs/architecture.md` and
`docs/implementation.md` for the rationale.

General rules:

- Shared DTOs live in `agent-fleet-protocol`; never redefine them in other crates.
- Errors: crates expose their own `thiserror` enum and `From<X> for ApiError`
  where the error crosses the RPC boundary.
- Async APIs use Tokio. Shared state is `Arc<...>`.
- No crate may create its own SQLite connection; only `agent-fleet-storage` does.
- No crate may run `git` except `agent-fleet-git-service`.
- No crate may spawn agent processes except `agent-fleet-agent-runtime`
  (through `agent-fleet-process-manager`).
- Unit tests live next to code; use `tempfile` for filesystem tests.

## 1. `agent-fleet-common`

Already implemented: IDs, `Timestamp`, `now()`, `new_id()`, `ApiError`,
`error_codes`, `Page<T>`, `Ack`, `init_tracing()`. Do not add business logic.

## 2. `agent-fleet-protocol`

Already implemented: unified DTOs and the connection envelope, plus
`protocol::rpc::methods` and request payloads. Do not add I/O logic.

## 3. `agent-fleet-process-manager`

```rust
pub use crate::error::ProcessError;
pub type ProcessResult<T> = Result<T, ProcessError>;

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub env: std::collections::BTreeMap<String, String>,
    pub clear_env: bool,
}
impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self;
    pub fn arg(self, arg: impl Into<String>) -> Self;
    pub fn args<I, S>(self, args: I) -> Self where I: IntoIterator<Item = S>, S: Into<String>;
    pub fn cwd(self, cwd: impl Into<std::path::PathBuf>) -> Self;
    pub fn env(self, key: impl Into<String>, value: impl Into<String>) -> Self;
}

#[derive(Debug, Clone)]
pub enum ProcessEvent {
    Started { pid: Option<u32> },
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exited { code: Option<i32>, success: bool },
}

pub struct ManagedProcess { /* ... */ }
impl ManagedProcess {
    pub async fn spawn(spec: CommandSpec) -> ProcessResult<Self>;
    pub fn pid(&self) -> Option<u32>;
    pub async fn write_stdin(&self, data: &[u8]) -> ProcessResult<()>;
    pub async fn close_stdin(&self) -> ProcessResult<()>;
    /// Subscribe to output events. Only one subscriber is required for MVP.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ProcessEvent>;
    /// Send SIGINT (Unix) / terminate.
    pub async fn interrupt(&self) -> ProcessResult<()>;
    pub async fn terminate(&self) -> ProcessResult<()>;
    pub async fn kill(&self) -> ProcessResult<()>;
    pub async fn wait(&mut self) -> ProcessResult<ExitStatus>;
    pub fn is_running(&self) -> bool;
}

pub struct CapturedOutput {
    pub status: Option<i32>,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}
/// Run a command to completion with timeout and output cap. Used by git-service.
pub async fn run_capture(
    spec: CommandSpec,
    timeout: std::time::Duration,
    max_output_bytes: usize,
) -> ProcessResult<CapturedOutput>;

pub struct PtyProcess { /* ... */ }
impl PtyProcess {
    pub fn spawn(spec: CommandSpec, cols: u16, rows: u16) -> ProcessResult<Self>;
    pub fn write(&mut self, data: &[u8]) -> ProcessResult<()>;
    pub fn resize(&mut self, cols: u16, rows: u16) -> ProcessResult<()>;
    pub fn reader(&self) -> tokio::sync::mpsc::Receiver<Vec<u8>>;
    pub fn kill(&mut self) -> ProcessResult<()>;
    pub fn wait(&mut self) -> ProcessResult<Option<i32>>;
}
```

Notes: children must be killed on drop or when the parent exits; on Unix put
the child in its own process group and kill the group.

## 4. `agent-fleet-config`

```rust
#[derive(Debug, Clone, Default)]
pub struct DaemonConfig {
    pub machine: MachineConfig,
    pub daemon: DaemonServerConfig,
    pub storage: StorageConfig,
    pub agent: AgentConfig,
    pub workspace: WorkspaceConfig,
    pub connection: ConnectionConfig,
}
pub struct MachineConfig { pub id: Option<String>, pub name: Option<String>, pub key_file: PathBuf }
pub struct DaemonServerConfig { pub host: String, pub port: u16 }          // 127.0.0.1:18080
pub struct StorageConfig { pub database: PathBuf }                          // ~/.agent-fleet/agent.db
pub struct AgentConfig { pub default_provider: String, pub claude_bin: String, pub codex_bin: String }
pub struct WorkspaceConfig { pub default_root: Option<PathBuf>, pub max_file_bytes: u64, pub default_ignores: Vec<String> }
pub struct ConnectionConfig { pub server_url: Option<String>, pub token: Option<String>, pub heartbeat_interval_ms: u64, pub reconnect_max_ms: u64 }

impl DaemonConfig {
    /// defaults < TOML file < `AGENT_FLEET_*` env vars
    pub fn load(path: Option<&std::path::Path>) -> anyhow::Result<Self>;
    pub fn from_str(toml_src: &str) -> anyhow::Result<Self>;
    pub fn resolve_paths(&mut self); // expand `~`, create parent dirs
}

#[derive(Debug, Clone, Default)]
pub struct ControlPlaneConfig {
    pub server: ServerConfig,       // host 127.0.0.1, port 8787, public_url
    pub storage: StorageConfig,     // ~/.agent-fleet/control-plane.db
    pub auth: AuthConfig,           // admin_token, heartbeat/request timeouts
    pub machines: MachinePolicy,    // heartbeat_interval_ms, heartbeat_timeout_ms, request_timeout_ms
    pub web: WebConfig,             // static_dir: Option<PathBuf>
}
impl ControlPlaneConfig { pub fn load(path: Option<&Path>) -> anyhow::Result<Self>; }
```

Environment variables: `AGENT_FLEET_SERVER_URL`, `AGENT_FLEET_TOKEN`,
`AGENT_FLEET_DB`, `AGENT_FLEET_DATA_DIR`, `AGENT_FLEET_ADMIN_TOKEN`,
`AGENT_FLEET_BIND`, `AGENT_FLEET_WEB_DIR`, `AGENT_FLEET_MACHINE_NAME`.

## 5. `agent-fleet-storage`

One SQLite pool, one embedded migration set at `migrations/schema` (repo root).

```rust
#[derive(Clone)]
pub struct Database { /* SqlitePool */ }
// `agent_fleet_storage::StorageError` is the crate error type; every method
// below returns `agent_fleet_storage::Result<T>`.
impl Database {
    pub async fn connect(path: impl AsRef<std::path::Path>) -> Result<Self>;
    pub async fn connect_in_memory() -> Result<Self>;
    pub async fn migrate(&self) -> Result<()>;
    pub fn pool(&self) -> &sqlx::SqlitePool;

    // machines (control plane)
    pub async fn upsert_machine(&self, m: &protocol::machine::Machine) -> Result<()>;
    pub async fn get_machine(&self, id: &str) -> Result<Option<Machine>>;
    pub async fn list_machines(&self) -> Result<Vec<Machine>>;
    pub async fn update_machine_status(&self, id: &str, status: MachineStatus,
        connected_at: Option<Timestamp>, last_seen_at: Option<Timestamp>) -> Result<()>;
    pub async fn delete_machine(&self, id: &str) -> Result<()>;

    // workspaces
    pub async fn upsert_workspace(&self, w: &protocol::workspace::Workspace) -> Result<()>;
    pub async fn get_workspace(&self, id: &str) -> Result<Option<Workspace>>;
    pub async fn list_workspaces(&self, machine_id: Option<&str>) -> Result<Vec<Workspace>>;
    pub async fn delete_workspace(&self, id: &str) -> Result<()>;

    // sessions
    pub async fn upsert_session(&self, s: &protocol::agent::AgentSession) -> Result<()>;
    pub async fn get_session(&self, id: &str) -> Result<Option<AgentSession>>;
    pub async fn list_sessions(&self, machine_id: Option<&str>) -> Result<Vec<AgentSession>>;
    pub async fn delete_session(&self, id: &str) -> Result<()>;

    // permissions
    pub async fn upsert_permission(&self, p: &AgentPermissionRequest) -> Result<()>;
    pub async fn get_permission(&self, id: &str) -> Result<Option<AgentPermissionRequest>>;
    pub async fn list_permissions(&self, session_id: &str) -> Result<Vec<AgentPermissionRequest>>;
    pub async fn list_pending_permissions(&self, session_id: &str) -> Result<Vec<AgentPermissionRequest>>;

    // workspace directory index
    pub async fn replace_entries(&self, workspace_id: &str, parent_path: &str,
        entries: &[WorkspaceEntry]) -> Result<()>;
    pub async fn list_entries(&self, workspace_id: &str, parent_path: &str)
        -> Result<Vec<WorkspaceEntry>>;
    pub async fn delete_entries_under(&self, workspace_id: &str, path_prefix: &str) -> Result<()>;

    // audit (control plane)
    pub async fn append_audit_log(&self, actor: &str, action: &str, subject: &str,
        details: serde_json::Value) -> Result<()>;
}
```

Notes: WAL mode, `busy_timeout=5000`, foreign keys on, max connections 8.
Sessions persist `metadata` as JSON text. All timestamps stored as RFC3339 text
(sortable) or integer millis — pick one and stay consistent.

## 6. `agent-fleet-auth`

```rust
#[derive(Clone)]
pub struct MachineIdentity { /* SigningKey */ }
impl MachineIdentity {
    pub fn generate() -> Self;
    pub fn load_or_create(path: &Path) -> Result<Self>;
    pub fn load(path: &Path) -> Result<Self>;
    pub fn save(&self, path: &Path) -> Result<()>;
    pub fn public_key_base64(&self) -> String;
    pub fn sign(&self, data: &[u8]) -> String;   // base64 signature
    pub fn fingerprint(&self) -> String;         // sha256 hex of public key
}

pub fn verify_signature(public_key_b64: &str, data: &[u8], signature_b64: &str) -> Result<()>;

#[derive(Clone)]
pub struct TokenAuth { /* token */ }
impl TokenAuth {
    pub fn new(token: impl Into<String>) -> Self;
    pub fn from_env() -> Result<Self>;
    pub fn verify_bearer(&self, header: Option<&str>) -> Result<()>;
    pub fn verify_raw(&self, token: &str) -> Result<()>;
}
pub struct Challenge { pub id: String, pub nonce: Vec<u8> }
pub fn new_challenge() -> Challenge;
pub fn nonce_base64(c: &Challenge) -> String;
```

The machine identity key file is `~/.agent-fleet/machine.key` (0600).

## 7. `agent-fleet-event-bus`

```rust
#[derive(Clone)]
pub struct EventBus { /* broadcast */ }
impl EventBus {
    pub fn new(capacity: usize) -> Self;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<platform::PlatformEvent>;
    pub fn publish(&self, event: PlatformEvent) -> usize;
}
```

## 8. `agent-fleet-workspace`

```rust
/// The workspace crate owns this settings type so it does not depend on
/// `agent-fleet-config`; apps convert their `WorkspaceConfig` into it.
#[derive(Debug, Clone)]
pub struct WorkspaceSettings {
    pub max_file_bytes: u64,
    pub default_ignores: Vec<String>,
}
impl Default for WorkspaceSettings { /* 2 MiB + standard ignores */ }

#[derive(Clone)]
pub struct WorkspaceService { /* storage + settings + watchers */ }
impl WorkspaceService {
    pub fn new(storage: Arc<Database>, settings: WorkspaceSettings) -> Self;
    pub async fn register(&self, machine_id: &str, req: RegisterWorkspaceRequest) -> Result<Workspace>;
    pub async fn list(&self, machine_id: Option<&str>) -> Result<Vec<Workspace>>;
    pub async fn get(&self, workspace_id: &str) -> Result<Workspace>;
    pub async fn remove(&self, workspace_id: &str) -> Result<()>;
    pub async fn list_directory(&self, workspace_id: &str, req: ListDirectoryRequest) -> Result<DirectoryListing>;
    pub async fn read_file(&self, workspace_id: &str, req: ReadFileRequest) -> Result<Option<FileContent>>; // None => not modified
    pub async fn search(&self, workspace_id: &str, req: SearchRequest) -> Result<SearchResponse>;
    /// Start a debounced watcher; events are published to the returned broadcast receiver.
    pub async fn watch(&self, workspace_id: &str) -> Result<WorkspaceWatcher>;
}
pub struct WorkspaceWatcher { pub events: tokio::sync::broadcast::Receiver<WorkspaceEvent> }
impl WorkspaceWatcher { pub async fn stop(self); }

pub struct PathPolicy { /* root */ }
impl PathPolicy {
    pub fn new(root: impl AsRef<Path>) -> Result<Self>;
    pub fn resolve(&self, rel: &str) -> Result<PathBuf>;     // denies .., absolute, symlink escape
    pub fn resolve_existing(&self, rel: &str) -> Result<PathBuf>;
    pub fn ensure_http_safe(&self) -> ... // optional
}

pub fn is_binary(bytes: &[u8]) -> bool;
pub fn detect_language(path: &str) -> Option<&'static str>;
```

Rules: default ignore list (`.git`, `node_modules`, `target`, `dist`, `.next`,
`__pycache__`, `.venv`) plus `.gitignore`; support `show_ignored`; sorted
directories first then name; cursor = last path (opaque base64 is fine);
`limit` default 200, max 1000; ETag = sha256 of content (or size+mtime for large).

## 9. `agent-fleet-git-service`

```rust
#[derive(Clone, Default)]
pub struct GitService { /* config */ }
impl GitService {
    pub fn new() -> Self;
    pub fn with_limits(max_output_bytes: usize, timeout: Duration) -> Self;
    pub async fn discover(&self, root: &Path) -> Result<Option<GitRepository>>;
    pub async fn status(&self, root: &Path) -> Result<GitStatus>;
    pub async fn diff(&self, root: &Path, options: &GitDiffOptions) -> Result<GitDiff>;
    pub async fn diff_summary(&self, root: &Path, options: &GitDiffOptions) -> Result<GitDiffSummary>;
    pub async fn branches(&self, root: &Path) -> Result<Vec<GitBranch>>;
    pub async fn log(&self, root: &Path, req: &GitLogRequest) -> Result<Vec<GitLogEntry>>;
}
pub fn parse_porcelain_v2(bytes: &[u8]) -> Result<GitStatus>;   // public for fixture tests
pub fn parse_unified_diff(path: &str, bytes: &[u8]) -> Result<FileDiff>; // public for fixture tests
```

Use `run_capture` with argument arrays only. `git status --porcelain=v2 -z --branch`.
Untracked files: read from disk with root-bound checks and size/binary limits,
emit as added-file diffs.

## 10. `agent-fleet-agent-runtime`

```rust
pub type AgentEventStream = futures::stream::BoxStream<'static, Result<AgentEvent>>;

#[derive(Clone)]
pub struct AgentEventEmitter { /* session_id, provider, seq, tx */ }
impl AgentEventEmitter {
    pub fn emit(&self, event_type: AgentEventType, payload: serde_json::Value);
    pub fn emit_with(&self, event_type: AgentEventType, payload: serde_json::Value,
        turn_id: Option<TurnId>, message_id: Option<MessageId>, tool_call_id: Option<String>);
    pub fn session_id(&self) -> &str;
}

pub struct ProviderContext {
    pub machine_id: String,
    pub session: AgentSession,
    pub emitter: AgentEventEmitter,
}

#[async_trait::async_trait]
pub trait AgentProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn info(&self) -> ProviderInfo;
    fn capabilities(&self) -> AgentCapabilities;
    async fn start(&self, ctx: ProviderContext, config: AgentStartConfig) -> Result<AgentSession>;
    async fn send_message(&self, session_id: &str, content: String) -> Result<()>;
    async fn interrupt(&self, session_id: &str) -> Result<()>;
    async fn resume(&self, session_id: &str) -> Result<()>;
    async fn shutdown(&self, session_id: &str) -> Result<()>;
    async fn respond_permission(&self, session_id: &str, decision: PermissionDecision) -> Result<()>;
}

pub struct ProviderRegistry { /* HashMap<String, Arc<dyn AgentProvider>> */ }
impl ProviderRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, provider: Arc<dyn AgentProvider>);
    pub fn get(&self, id: &str) -> Option<Arc<dyn AgentProvider>>;
    pub fn infos(&self) -> Vec<ProviderInfo>;
    pub fn default_provider(&self) -> Option<Arc<dyn AgentProvider>>;
}

pub struct AgentRuntime { /* registry, sessions, event bus, storage */ }
impl AgentRuntime {
    pub fn new(registry: ProviderRegistry, event_capacity: usize) -> Self;
    pub fn with_storage(self, storage: Arc<Database>) -> Self;
    pub fn providers(&self) -> Vec<ProviderInfo>;
    pub async fn start_session(&self, machine_id: &str, provider: &str,
        config: AgentStartConfig) -> Result<AgentSession>;
    pub async fn send_message(&self, session_id: &str, req: SendMessageRequest) -> Result<()>;
    pub async fn interrupt(&self, session_id: &str) -> Result<()>;
    pub async fn resume(&self, session_id: &str) -> Result<()>;
    pub async fn shutdown_session(&self, session_id: &str) -> Result<()>;
    pub async fn shutdown_all(&self);
    pub async fn respond_permission(&self, decision: PermissionDecision) -> Result<()>;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent>;
    pub async fn list_sessions(&self) -> Vec<AgentSession>;
    pub async fn get_session(&self, id: &str) -> Option<AgentSession>;
    pub async fn session_detail(&self, id: &str) -> Option<SessionDetail>;
}
```

Adapters: `claude`, `codex`, `mock`, `generic`. Mock must be fully functional
for E2E tests. Claude and Codex must parse documented/fixture protocols and
declare honest capabilities. Runtime owns session state machine transitions and
forwards events to subscribers; `metadata.provider_session_id` stores native IDs.

## 11. `agent-fleet-connection-gateway`

```rust
#[derive(Clone)]
pub struct ConnectionRegistry { /* DashMap<MachineId, ConnectionHandle> */ }
impl ConnectionRegistry {
    pub fn new() -> Self;
    pub fn is_online(&self, machine_id: &str) -> bool;
    pub fn online(&self) -> Vec<ConnectionInfo>;
    pub async fn request(&self, machine_id: &str, method: &str,
        payload: serde_json::Value) -> Result<serde_json::Value, ApiError>;
    pub async fn notify(&self, machine_id: &str, event: &str, payload: serde_json::Value) -> Result<(), ApiError>;
    pub fn disconnect(&self, machine_id: &str);
}

#[async_trait]
pub trait MachineAuthenticator: Send + Sync {
    async fn pinned_key(&self, machine_id: &str) -> Result<Option<String>, ApiError>;
    async fn pin_key(&self, machine_id: &str, public_key: &str) -> Result<(), ApiError>;
}

pub enum GatewayEvent {
    MachineConnected { machine_id: String, hello: HelloPayload, connection_id: String },
    MachineDisconnected { machine_id: String, reason: Option<String> },
    MachineEvent { machine_id: String, event: String, payload: serde_json::Value },
    Heartbeat { machine_id: String, at: Timestamp },
}
pub struct ConnectionGateway { /* registry, auth, events */ }
impl ConnectionGateway {
    pub fn new(token_auth: TokenAuth, authenticator: Arc<dyn MachineAuthenticator>,
        heartbeat_interval: Duration, heartbeat_timeout: Duration) -> Arc<Self>;
    pub fn registry(&self) -> ConnectionRegistry;
    pub fn subscribe(&self) -> broadcast::Receiver<GatewayEvent>;
    /// Axum router: `GET /machine` upgrades to a machine WebSocket.
    pub fn router(self: std::sync::Arc<Self>) -> axum::Router;
}

#[async_trait]
pub trait DaemonRequestHandler: Send + Sync {
    async fn handle(&self, method: &str, payload: serde_json::Value) -> Result<serde_json::Value, ApiError>;
    async fn on_connected(&self, _accepted: &ConnectionAccepted) {}
    async fn on_disconnected(&self, _reason: &str) {}
}
pub struct MachineConnectionClient { /* ... */ }
impl MachineConnectionClient {
    pub fn new(config: ClientConfig, identity: MachineIdentity,
        handler: Arc<dyn DaemonRequestHandler>) -> Self;
    pub fn state(&self) -> ConnectionState;
    /// Runs the connect/authenticate/heartbeat/reconnect loop until cancellation.
    pub async fn run(self: Arc<Self>, shutdown: tokio_util::sync::CancellationToken);
}
```

Server flow: verify bearer token on upgrade → hello → challenge (Ed25519) →
accepted → heartbeats → request/response correlation by `message_id`.
Unknown machines may register on first connect only if the user token is valid
(trust-on-first-use, key stored by the control plane).

## 12. Apps

`apps/agent-daemon`: CLI (`serve`, `register-workspace`, `providers`), builds
services, implements `DaemonRequestHandler` by dispatching `protocol::rpc::methods`
to workspace/git/runtime, runs the connection client with backoff, exposes
`GET /health` on 127.0.0.1:18080, graceful shutdown, kills child processes.

`apps/control-plane`: Axum server on 127.0.0.1:8787, bearer auth middleware,
machine WebSocket at `/ws/machine`, browser events at `/api/v1/events`,
REST proxy endpoints per `docs/architecture.md` §13, serves `apps/web/dist`
when present, CORS for `http://localhost:5173`.

REST mapping (all under `/api/v1`):

| Method | Path | Daemon method |
|---|---|---|
| GET | `/machines` | local DB |
| GET | `/machines/:id` | local DB |
| DELETE | `/machines/:id` | local DB |
| GET | `/machines/:id/providers` | `agent.providers.list` |
| GET | `/workspaces?machine_id=` | local DB |
| GET | `/workspaces/:id` | local DB |
| POST | `/workspaces` | `workspace.register` (body has `machine_id`) |
| DELETE | `/workspaces/:id` | `workspace.remove` |
| GET | `/workspaces/:id/tree` | `workspace.tree.list` |
| GET | `/workspaces/:id/files` | `workspace.file.read` |
| GET | `/workspaces/:id/search` | `workspace.file.search` |
| GET | `/workspaces/:id/git/status` | `git.status` |
| GET | `/workspaces/:id/git/diff` | `git.diff` |
| GET | `/workspaces/:id/git/diff/summary` | `git.diff.summary` |
| GET | `/workspaces/:id/git/branches` | `git.branches` |
| GET | `/workspaces/:id/git/log` | `git.log` |
| GET | `/sessions` | local DB |
| POST | `/sessions` | `agent.sessions.create` |
| GET | `/sessions/:id` | `agent.sessions.get` |
| POST | `/sessions/:id/messages` | `agent.sessions.send_message` |
| POST | `/sessions/:id/interrupt` | `agent.sessions.interrupt` |
| POST | `/sessions/:id/resume` | `agent.sessions.resume` |
| DELETE | `/sessions/:id` | `agent.sessions.shutdown` |
| GET | `/sessions/:id/permissions` | `agent.permissions.list` |
| POST | `/permissions/:id/approve` | `agent.permissions.respond` |
| POST | `/permissions/:id/reject` | `agent.permissions.respond` |

Control plane receives daemon events (`agent.event`, `workspace.event`,
`git.status.changed`) and republishes them as `PlatformEvent` on
`/api/v1/events` and the `EventBus`.

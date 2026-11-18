//! Per-session ACP state.
//!
//! The session handle owns the connection, the mapper state and the native
//! session id. The driver only keeps a registry of these handles; it never
//! stores mutable protocol state for a session itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::task::JoinHandle;

use crate::driver::acp::config::AcpProtocolVersion;
use crate::driver::acp::connection::AcpConnection;
use crate::driver::acp::mapper::v1::SessionUpdateState as V1UpdateState;
use crate::driver::acp::mapper::AcpUpdateState;
use crate::event::AgentEventEmitter;

/// Mutable ACP state for one Fleet session.
pub struct AcpSessionHandle {
    /// Fleet session id.
    pub fleet_session_id: String,
    /// Fleet instance id.
    pub instance_id: String,
    /// Negotiated protocol version, set once the handshake completes.
    pub protocol: Mutex<Option<AcpProtocolVersion>>,
    /// Native ACP session id.
    pub native_session_id: Mutex<Option<String>>,
    /// Agent implementation info from `initialize`.
    pub agent_info: Mutex<serde_json::Value>,
    /// Agent capabilities from `initialize`.
    pub capabilities: Mutex<fleet_core_protocol::agent::AgentCapabilities>,
    /// Launch definition snapshot, used to respawn for a resume.
    pub definition: Mutex<Option<crate::agent::AgentDefinition>>,
    /// Session working directory; the v1 filesystem client's sandbox root.
    pub workspace_root: Mutex<String>,
    /// Canonical session working directory passed to `session/new`.
    pub session_cwd: Mutex<String>,
    /// Effective launch environment, kept in memory so a dead process can be
    /// respawned for `session/resume`. Never persisted.
    pub launch_env: Mutex<std::collections::BTreeMap<String, String>>,
    /// Live v1 terminals owned by this session.
    pub terminals: crate::driver::acp::terminal::TerminalManager,
    /// MCP server descriptors supplied for this session (never persisted).
    pub mcp_servers: Mutex<Vec<serde_json::Value>>,
    /// Serializes dead-process resume so concurrent RPCs cannot spawn two
    /// agent processes for one session.
    pub resume_lock: tokio::sync::Mutex<()>,
    /// v1 mapper state.
    pub v1_state: Mutex<V1UpdateState>,
    /// v2 mapper state.
    pub v2_state: Mutex<AcpUpdateState>,
    /// Live connection handle.
    pub connection: Mutex<Option<Arc<AcpConnection>>>,
    /// Connection task handle.
    pub task: Mutex<Option<JoinHandle<()>>>,
    /// Event sink.
    pub emitter: AgentEventEmitter,
    /// Set before an intentional shutdown so the connection task does not
    /// report the session as failed.
    pub closing: AtomicBool,
    /// How long a permission request may stay unanswered.
    pub permission_timeout: std::time::Duration,
    /// How long a question may stay unanswered.
    pub question_timeout: std::time::Duration,
}

impl AcpSessionHandle {
    /// Create a handle before the connection is established.
    pub fn new(
        fleet_session_id: impl Into<String>,
        instance_id: impl Into<String>,
        emitter: AgentEventEmitter,
        permission_timeout: std::time::Duration,
        question_timeout: std::time::Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            fleet_session_id: fleet_session_id.into(),
            instance_id: instance_id.into(),
            protocol: Mutex::new(None),
            native_session_id: Mutex::new(None),
            agent_info: Mutex::new(serde_json::Value::Null),
            capabilities: Mutex::new(Default::default()),
            definition: Mutex::new(None),
            workspace_root: Mutex::new(String::new()),
            session_cwd: Mutex::new(String::new()),
            launch_env: Mutex::new(std::collections::BTreeMap::new()),
            terminals: crate::driver::acp::terminal::TerminalManager::new(),
            mcp_servers: Mutex::new(Vec::new()),
            resume_lock: tokio::sync::Mutex::new(()),
            v1_state: Mutex::new(V1UpdateState::default()),
            v2_state: Mutex::new(AcpUpdateState::default()),
            connection: Mutex::new(None),
            task: Mutex::new(None),
            emitter,
            closing: AtomicBool::new(false),
            permission_timeout,
            question_timeout,
        })
    }

    /// Whether a shutdown was intentionally requested.
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    /// Mark the session as intentionally closing.
    pub fn begin_shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
}

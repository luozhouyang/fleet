//! Machine-event pipeline: gateway events in, [`PlatformEvent`]s out.
//!
//! The task is deliberately forgiving: a malformed event is logged and
//! skipped, never allowed to terminate the consumer.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashmap::DashMap;
use fleet_core_common::now;
use fleet_core_connection::GatewayEvent;
use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentPermissionRequest, AgentSession, PermissionStatus,
    ProviderInfo, SessionStatus, StatusChangedPayload,
};
use fleet_core_protocol::connection::{events as connection_events, PlatformEvent};
use fleet_core_protocol::machine::MachineRegistration;
use fleet_core_protocol::rpc::{methods, AckEventsRequest};
use fleet_core_protocol::workspace::{Workspace, WorkspaceEvent, WorkspaceSummary};
use serde_json::{json, Value};

use crate::state::AppState;

/// Where a permission request should be routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRoute {
    /// Session that owns the permission request.
    pub session_id: String,
    /// Machine that owns the session.
    pub machine_id: String,
}

/// In-memory indexes maintained while consuming machine events.
///
/// They complement the database: the permission index lets the approval route
/// deliver a decision even when the permission row has not been persisted yet.
#[derive(Clone)]
pub struct EventIndexes {
    sequences: Arc<DashMap<String, AtomicU64>>,
    permissions: Arc<DashMap<String, PermissionRoute>>,
    machine_orgs: Arc<DashMap<String, String>>,
    seen_agent_events: Arc<Mutex<SeenEvents>>,
}

struct SeenEvents {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

const SEEN_AGENT_EVENT_LIMIT: usize = 8192;

impl Default for EventIndexes {
    fn default() -> Self {
        Self::new()
    }
}

impl EventIndexes {
    /// Create empty indexes.
    pub fn new() -> Self {
        Self {
            sequences: Arc::new(DashMap::new()),
            permissions: Arc::new(DashMap::new()),
            machine_orgs: Arc::new(DashMap::new()),
            seen_agent_events: Arc::new(Mutex::new(SeenEvents {
                ids: HashSet::new(),
                order: VecDeque::new(),
            })),
        }
    }

    /// Next per-machine sequence number (starting at 1).
    pub fn next_sequence(&self, machine_id: &str) -> u64 {
        let counter = self
            .sequences
            .entry(machine_id.to_string())
            .or_insert_with(|| AtomicU64::new(0));
        counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Remember which session/machine owns a permission request.
    pub fn record_permission(&self, permission_id: &str, session_id: &str, machine_id: &str) {
        self.permissions.insert(
            permission_id.to_string(),
            PermissionRoute {
                session_id: session_id.to_string(),
                machine_id: machine_id.to_string(),
            },
        );
    }

    /// Look up the route for a permission request.
    pub fn permission_route(&self, permission_id: &str) -> Option<PermissionRoute> {
        self.permissions
            .get(permission_id)
            .map(|entry| entry.value().clone())
    }

    /// Cache the organization that owns a machine.
    pub fn set_machine_org(&self, machine_id: &str, org_id: &str) {
        self.machine_orgs
            .insert(machine_id.to_string(), org_id.to_string());
    }

    /// Record a durable agent event id. Returns `false` when the event was
    /// already applied (for example a replay after an unacknowledged write).
    pub fn mark_agent_event(&self, event_id: &str) -> bool {
        let mut seen = self
            .seen_agent_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !seen.ids.insert(event_id.to_string()) {
            return false;
        }
        seen.order.push_back(event_id.to_string());
        while seen.order.len() > SEEN_AGENT_EVENT_LIMIT {
            if let Some(expired) = seen.order.pop_front() {
                seen.ids.remove(&expired);
            }
        }
        true
    }

    /// Cached machine ownership, if known.
    pub fn machine_org(&self, machine_id: &str) -> Option<String> {
        self.machine_orgs
            .get(machine_id)
            .map(|entry| entry.value().clone())
    }
}

/// Spawn the gateway-event consumer. The task runs until the gateway closes.
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.gateway.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => handle_gateway_event(&state, event).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "machine event consumer lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        tracing::debug!("machine event consumer stopped");
    })
}

async fn handle_gateway_event(state: &AppState, event: GatewayEvent) {
    match event {
        GatewayEvent::MachineConnected {
            machine_id,
            hello,
            connection_id,
        } => on_machine_connected(state, machine_id, hello, connection_id).await,
        GatewayEvent::MachineDisconnected {
            machine_id,
            connection_id,
            ..
        } => {
            // A replacement connection may already be live; do not let the
            // replaced socket's cleanup mark the machine offline.
            if state.connections.is_online(&machine_id) {
                tracing::debug!(
                    machine = %machine_id,
                    connection_id = %connection_id,
                    "ignoring stale disconnect for a live replacement connection"
                );
            } else {
                on_machine_disconnected(state, &machine_id).await;
            }
        }
        GatewayEvent::Heartbeat { machine_id, .. } => {
            if let Err(err) = state.machines.touch(&machine_id).await {
                tracing::debug!(machine = %machine_id, error = %err, "heartbeat touch failed");
            }
        }
        GatewayEvent::MachineEvent {
            machine_id,
            event,
            sequence,
            cursor,
            payload,
        } => handle_machine_event(state, &machine_id, &event, sequence, cursor, payload).await,
    }
}

async fn on_machine_connected(
    state: &AppState,
    machine_id: String,
    hello: fleet_core_protocol::connection::HelloPayload,
    connection_id: String,
) {
    // A revocation that races with an already in-flight MachineConnected event
    // must win. The handshake also rejects revoked machines, but this protects
    // against an event queued just before the revocation was committed.
    if state
        .db
        .machines
        .is_revoked(&machine_id)
        .await
        .unwrap_or(false)
    {
        tracing::warn!(machine = %machine_id, "ignoring connection event for a revoked machine");
        state.connections.disconnect(&machine_id);
        return;
    }
    if let Err(err) = state
        .db
        .machine_orgs
        .ensure(&machine_id, fleet_core_protocol::authz::DEFAULT_ORG_ID)
        .await
    {
        tracing::warn!(machine = %machine_id, error = %err, "failed to assign default organization");
    }
    if let Ok(Some(org_id)) = state.db.machine_orgs.get(&machine_id).await {
        state.event_index.set_machine_org(&machine_id, &org_id);
    }

    // Register the connection immediately so request routing works, reusing
    // the previous snapshot until the asynchronous refresh finishes. Blocking
    // this consumer on daemon snapshot RPCs can overflow the gateway event
    // channel during a reconnect storm.
    let existing = state.machines.get(&machine_id).await.ok().flatten();
    let providers = existing
        .as_ref()
        .map(|machine| machine.providers.clone())
        .unwrap_or_default();
    let workspaces = existing
        .as_ref()
        .map(|machine| machine.workspaces.clone())
        .unwrap_or_default();
    register_machine(
        state,
        &machine_id,
        &hello,
        providers,
        workspaces,
        &connection_id,
    )
    .await;

    let state = state.clone();
    tokio::spawn(async move {
        refresh_machine_snapshot(state, machine_id, hello, connection_id).await;
    });
}

/// Register (or refresh) machine metadata and publish a status event.
async fn register_machine(
    state: &AppState,
    machine_id: &str,
    hello: &fleet_core_protocol::connection::HelloPayload,
    providers: Vec<ProviderInfo>,
    workspaces: Vec<WorkspaceSummary>,
    connection_id: &str,
) {
    let registration = MachineRegistration {
        machine_id: machine_id.to_string(),
        name: hello.name.clone(),
        hostname: hello.hostname.clone(),
        os: hello.os.clone(),
        arch: hello.arch.clone(),
        daemon_version: hello.daemon_version.clone(),
        protocol_version: hello.protocol_version,
        public_key: hello.public_key.clone(),
        providers,
        workspaces,
    };

    let machine = match state.machines.register_or_update(registration).await {
        Ok(machine) => {
            if let Err(err) = state.machines.set_online(machine_id).await {
                tracing::warn!(machine = %machine_id, error = %err, "failed to mark machine online");
            }
            Some(machine)
        }
        Err(err) => {
            tracing::warn!(machine = %machine_id, error = %err, "machine registration failed");
            state.machines.get(machine_id).await.ok().flatten()
        }
    };

    let payload = machine
        .map(|machine| serde_json::to_value(machine).unwrap_or(Value::Null))
        .unwrap_or_else(|| json!({"id": machine_id, "status": "online"}));
    let mut status_event = PlatformEvent::new(connection_events::MACHINE_STATUS, payload);
    status_event.machine_id = Some(machine_id.to_string());
    state.events.publish(status_event);

    let mut connection_event = PlatformEvent::new(
        connection_events::CONNECTION_STATE,
        json!({
            "machine_id": machine_id,
            "connection_id": connection_id,
            "state": "online",
        }),
    );
    connection_event.machine_id = Some(machine_id.to_string());
    state.events.publish(connection_event);
}

/// Best-effort provider/workspace snapshot refresh after registration.
async fn refresh_machine_snapshot(
    state: AppState,
    machine_id: String,
    hello: fleet_core_protocol::connection::HelloPayload,
    connection_id: String,
) {
    let timeout = Duration::from_millis(state.config.machines.request_timeout_ms.clamp(500, 5_000));
    let providers = match state
        .connections
        .request_with_timeout(&machine_id, methods::PROVIDERS_LIST, json!({}), timeout)
        .await
    {
        Ok(value) => serde_json::from_value::<Vec<ProviderInfo>>(value).unwrap_or_else(|err| {
            tracing::warn!(machine = %machine_id, error = %err, "invalid provider snapshot");
            Vec::new()
        }),
        Err(err) => {
            tracing::debug!(machine = %machine_id, code = %err.code, "provider snapshot unavailable");
            Vec::new()
        }
    };

    let workspaces = match state
        .connections
        .request_with_timeout(&machine_id, methods::WORKSPACES_LIST, json!({}), timeout)
        .await
    {
        Ok(value) => serde_json::from_value::<Vec<Workspace>>(value).unwrap_or_else(|err| {
            tracing::warn!(machine = %machine_id, error = %err, "invalid workspace snapshot");
            Vec::new()
        }),
        Err(err) => {
            tracing::debug!(machine = %machine_id, code = %err.code, "workspace snapshot unavailable");
            Vec::new()
        }
    };

    for workspace in &workspaces {
        if let Err(err) = state.db.workspaces.upsert(workspace).await {
            tracing::warn!(
                machine = %machine_id,
                workspace = %workspace.id,
                error = %err,
                "failed to persist workspace snapshot"
            );
        }
    }

    // Do not let a late snapshot from a replaced connection overwrite the
    // metadata of the connection that is actually live.
    let current = state.connections.current_connection_id(&machine_id);
    if current.as_deref() != Some(connection_id.as_str()) {
        return;
    }

    let summaries: Vec<WorkspaceSummary> = workspaces.iter().map(WorkspaceSummary::from).collect();
    register_machine(
        &state,
        &machine_id,
        &hello,
        providers,
        summaries,
        &connection_id,
    )
    .await;
}

async fn on_machine_disconnected(state: &AppState, machine_id: &str) {
    // Revocation deletes the machine row and disconnects it. The resulting
    // disconnect event can arrive afterwards; it must not recreate the row.
    if state
        .db
        .machines
        .is_revoked(machine_id)
        .await
        .unwrap_or(false)
    {
        return;
    }
    let payload = match state.machines.set_offline(machine_id).await {
        Ok(machine) => serde_json::to_value(machine).unwrap_or(Value::Null),
        Err(err) => {
            tracing::warn!(machine = %machine_id, error = %err, "failed to mark machine offline");
            json!({"id": machine_id, "status": "offline"})
        }
    };
    let mut event = PlatformEvent::new(connection_events::MACHINE_STATUS, payload);
    event.machine_id = Some(machine_id.to_string());
    state.events.publish(event);
}

async fn handle_machine_event(
    state: &AppState,
    machine_id: &str,
    event_name: &str,
    sequence: u64,
    cursor: Option<u64>,
    payload: Value,
) {
    let mut platform = base_platform_event(machine_id, event_name, payload.clone());
    platform.sequence = if sequence > 0 {
        sequence
    } else {
        state.event_index.next_sequence(machine_id)
    };
    let mut duplicate = false;

    match event_name {
        connection_events::AGENT_EVENT => match serde_json::from_value::<AgentEvent>(payload) {
            Ok(agent_event) => {
                platform.session_id = Some(agent_event.session_id.clone());
                // Only durable frames are replayed. Ephemeral deltas are
                // live-only, so they must not evict durable ids from the
                // bounded deduplication set.
                if cursor.is_some() && !state.event_index.mark_agent_event(&agent_event.event_id) {
                    duplicate = true;
                } else {
                    apply_agent_event(state, machine_id, &agent_event).await;
                }
            }
            Err(err) => {
                tracing::warn!(
                    machine = %machine_id,
                    error = %err,
                    "malformed agent.event payload"
                );
            }
        },
        connection_events::WORKSPACE_EVENT => {
            match serde_json::from_value::<WorkspaceEvent>(payload) {
                Ok(workspace_event) => {
                    platform.workspace_id = Some(workspace_event.workspace_id);
                }
                Err(err) => {
                    tracing::warn!(
                        machine = %machine_id,
                        error = %err,
                        "malformed workspace.event payload"
                    );
                }
            }
        }
        connection_events::GIT_STATUS_CHANGED => {
            platform.workspace_id = workspace_id_from_payload(&platform.payload);
        }
        _ => {}
    }

    if !duplicate {
        state.events.publish(platform);
    }

    if let Some(cursor) = cursor {
        acknowledge_daemon_event(state, machine_id, cursor);
    }
}

/// Confirm delivery of a durable daemon event. Acknowledgement failures are
/// harmless: the event stays in the outbox and is replayed later.
fn acknowledge_daemon_event(state: &AppState, machine_id: &str, cursor: u64) {
    let state = state.clone();
    let machine_id = machine_id.to_string();
    tokio::spawn(async move {
        let payload = serde_json::to_value(AckEventsRequest { cursor }).unwrap_or(Value::Null);
        if let Err(error) = state
            .connections
            .request(&machine_id, methods::EVENTS_ACK, payload)
            .await
        {
            tracing::debug!(
                machine = %machine_id,
                cursor,
                error = %error,
                "failed to acknowledge daemon event"
            );
        }
    });
}

/// Build the platform envelope for a daemon event.
fn base_platform_event(machine_id: &str, event_name: &str, payload: Value) -> PlatformEvent {
    let mut event = PlatformEvent::new(event_name, payload);
    event.machine_id = Some(machine_id.to_string());
    event
}

async fn apply_agent_event(state: &AppState, machine_id: &str, event: &AgentEvent) {
    match event.event_type {
        AgentEventType::SessionCreated => {
            if let Some(session) = parse_agent_session(&event.payload) {
                if let Err(err) = state.db.sessions.upsert(&session).await {
                    tracing::warn!(session = %session.id, error = %err, "failed to upsert session");
                }
            } else {
                patch_session(
                    state,
                    &event.session_id,
                    None,
                    title_from_payload(&event.payload),
                )
                .await;
            }
        }
        // Transcript rows are owned and persisted by the daemon. The admin
        // forwards these events to browsers but does not mirror them.
        AgentEventType::TurnStarted
        | AgentEventType::TurnCompleted
        | AgentEventType::TurnInterrupted
        | AgentEventType::TurnFailed
        | AgentEventType::MessageCompleted => {}
        AgentEventType::SessionDeleted => {
            if let Err(err) = state.db.sessions.delete(&event.session_id).await {
                tracing::warn!(session = %event.session_id, error = %err, "failed to delete session after daemon event");
            }
        }
        AgentEventType::AgentStatusChanged => {
            patch_session(
                state,
                &event.session_id,
                parse_status_changed(&event.payload),
                title_from_payload(&event.payload),
            )
            .await;
        }
        AgentEventType::PermissionRequested
        | AgentEventType::PermissionApproved
        | AgentEventType::PermissionRejected => {
            let Some(mut permission) = parse_permission(&event.payload) else {
                tracing::warn!(
                    machine = %machine_id,
                    session = %event.session_id,
                    "permission event without a parseable permission payload"
                );
                return;
            };

            match event.event_type {
                AgentEventType::PermissionApproved => {
                    permission.status = PermissionStatus::Approved;
                    permission.resolved_at = Some(now());
                }
                AgentEventType::PermissionRejected => {
                    permission.status = PermissionStatus::Rejected;
                    permission.resolved_at = Some(now());
                }
                _ => {}
            }

            if permission.status == PermissionStatus::Pending {
                state.event_index.record_permission(
                    &permission.id,
                    &permission.session_id,
                    machine_id,
                );
            }

            if let Err(err) = state.db.permissions.upsert(&permission).await {
                tracing::warn!(
                    permission = %permission.id,
                    error = %err,
                    "failed to persist permission"
                );
            }
        }
        _ => {}
    }
}

async fn patch_session(
    state: &AppState,
    session_id: &str,
    status: Option<SessionStatus>,
    title: Option<String>,
) {
    match state.db.sessions.get(session_id).await {
        Ok(Some(mut session)) => {
            if let Some(status) = status {
                session.status = status;
            }
            if let Some(title) = title {
                if !title.is_empty() {
                    session.title = Some(title);
                }
            }
            session.updated_at = now();
            if let Err(err) = state.db.sessions.upsert(&session).await {
                tracing::warn!(session = %session_id, error = %err, "failed to update session");
            }
        }
        Ok(None) => tracing::debug!(
            session = %session_id,
            "event for a session that has no local row yet"
        ),
        Err(err) => {
            tracing::warn!(session = %session_id, error = %err, "failed to read session");
        }
    }
}

fn parse_agent_session(value: &Value) -> Option<AgentSession> {
    serde_json::from_value::<AgentSession>(value.clone())
        .ok()
        .or_else(|| {
            value
                .get("session")
                .and_then(|inner| serde_json::from_value::<AgentSession>(inner.clone()).ok())
        })
}

fn parse_status_changed(value: &Value) -> Option<SessionStatus> {
    serde_json::from_value::<StatusChangedPayload>(value.clone())
        .ok()
        .map(|payload| payload.status)
        .or_else(|| {
            value
                .get("status")
                .and_then(|inner| serde_json::from_value::<SessionStatus>(inner.clone()).ok())
        })
}

fn parse_permission(value: &Value) -> Option<AgentPermissionRequest> {
    serde_json::from_value::<AgentPermissionRequest>(value.clone())
        .ok()
        .or_else(|| {
            value.get("permission").and_then(|inner| {
                serde_json::from_value::<AgentPermissionRequest>(inner.clone()).ok()
            })
        })
}

fn title_from_payload(value: &Value) -> Option<String> {
    value
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .get("session")
                .and_then(|session| session.get("title"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn workspace_id_from_payload(payload: &Value) -> Option<String> {
    payload
        .get("workspace_id")
        .and_then(Value::as_str)
        .or_else(|| payload.get("workspaceId").and_then(Value::as_str))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state_with_online_machine(machine_id: &str) -> AppState {
        let db = Arc::new(
            fleet_admin_storage::AdminStorage::connect_in_memory()
                .await
                .unwrap(),
        );
        let config = fleet_core_config::AdminConfig::default();
        let state = AppState::new(db, config).await.unwrap();
        state
            .machines
            .register_or_update(MachineRegistration {
                machine_id: machine_id.into(),
                name: "test".into(),
                hostname: "host".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                daemon_version: "0.1.0".into(),
                protocol_version: 1,
                public_key: "key".into(),
                providers: vec![],
                workspaces: vec![],
            })
            .await
            .unwrap();
        state
    }

    #[tokio::test]
    async fn disconnect_event_does_not_touch_a_revoked_machine() {
        let state = state_with_online_machine("m_revoked").await;
        state
            .db
            .machines
            .revoke("m_revoked", Some("test"))
            .await
            .unwrap();

        on_machine_disconnected(&state, "m_revoked").await;

        let machine = state.machines.get("m_revoked").await.unwrap().unwrap();
        assert_eq!(
            machine.status,
            fleet_core_protocol::machine::MachineStatus::Online
        );
    }

    #[tokio::test]
    async fn transcript_events_are_not_mirrored_into_admin_storage() {
        let state = state_with_online_machine("m_mirror").await;
        let session =
            fleet_core_protocol::agent::AgentSession::new("m_mirror", "mock", "mock", "/tmp");
        state.db.sessions.upsert(&session).await.unwrap();
        let message = fleet_core_protocol::agent::AgentMessage {
            id: "message_user".into(),
            session_id: session.id.clone(),
            turn_id: Some("turn_1".into()),
            role: fleet_core_protocol::agent::MessageRole::User,
            content: "hello".into(),
            created_at: now(),
        };
        let mut event = AgentEvent::new(
            session.id.clone(),
            1,
            AgentEventType::MessageCompleted,
            "mock",
        );
        event.payload = serde_json::to_value(fleet_core_protocol::agent::MessageCompletedPayload {
            message: message.clone(),
        })
        .unwrap();

        apply_agent_event(&state, "m_mirror", &event).await;

        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_messages'",
        )
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn replayed_agent_events_are_deduplicated_by_event_id() {
        let indexes = EventIndexes::new();
        assert!(indexes.mark_agent_event("evt_1"));
        assert!(!indexes.mark_agent_event("evt_1"));
        assert!(indexes.mark_agent_event("evt_2"));
    }

    #[test]
    fn sequences_are_monotonic_per_machine() {
        let indexes = EventIndexes::new();
        assert_eq!(indexes.next_sequence("m1"), 1);
        assert_eq!(indexes.next_sequence("m1"), 2);
        assert_eq!(indexes.next_sequence("m2"), 1);
        assert_eq!(indexes.next_sequence("m1"), 3);
    }

    #[test]
    fn permission_index_routes_requests() {
        let indexes = EventIndexes::new();
        indexes.record_permission("perm_1", "session_1", "machine_1");
        assert_eq!(
            indexes.permission_route("perm_1"),
            Some(PermissionRoute {
                session_id: "session_1".into(),
                machine_id: "machine_1".into(),
            })
        );
        assert_eq!(indexes.permission_route("perm_2"), None);
    }

    #[test]
    fn platform_event_builder_attaches_machine_and_type() {
        let event = base_platform_event("m1", "agent.event", json!({"n": 1}));
        assert_eq!(event.event_type, "agent.event");
        assert_eq!(event.machine_id.as_deref(), Some("m1"));
        assert_eq!(event.payload, json!({"n": 1}));
        assert_eq!(event.sequence, 0);
    }

    #[test]
    fn payload_parsers_tolerate_malformed_values() {
        assert!(parse_agent_session(&json!({"unexpected": true})).is_none());
        assert!(parse_status_changed(&json!({"unexpected": true})).is_none());
        assert!(parse_permission(&json!({"unexpected": true})).is_none());
        assert_eq!(title_from_payload(&json!({"unexpected": true})), None);
        assert_eq!(
            workspace_id_from_payload(&json!({"workspaceId": "w1"})),
            Some("w1".to_string())
        );
    }

    #[test]
    fn status_payload_variants_are_accepted() {
        let nested = json!({"status": "running"});
        assert_eq!(parse_status_changed(&nested), Some(SessionStatus::Running));

        let full = json!({"status": "waiting_for_permission", "reason": "tool"});
        assert_eq!(
            parse_status_changed(&full),
            Some(SessionStatus::WaitingForPermission)
        );
    }
}

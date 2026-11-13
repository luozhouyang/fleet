//! Machine-event pipeline: gateway events in, [`PlatformEvent`]s out.
//!
//! The task is deliberately forgiving: a malformed event is logged and
//! skipped, never allowed to terminate the consumer.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_fleet_common::now;
use agent_fleet_connection_gateway::GatewayEvent;
use agent_fleet_protocol::agent::{
    AgentEvent, AgentEventType, AgentPermissionRequest, AgentSession, PermissionStatus,
    ProviderInfo, SessionStatus, StatusChangedPayload,
};
use agent_fleet_protocol::connection::{events as connection_events, PlatformEvent};
use agent_fleet_protocol::machine::MachineRegistration;
use agent_fleet_protocol::rpc::methods;
use agent_fleet_protocol::workspace::{Workspace, WorkspaceEvent, WorkspaceSummary};
use dashmap::DashMap;
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
}

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
        GatewayEvent::MachineDisconnected { machine_id, .. } => {
            on_machine_disconnected(state, &machine_id).await;
        }
        GatewayEvent::Heartbeat { machine_id, .. } => {
            if let Err(err) = state.machines.touch(&machine_id).await {
                tracing::debug!(machine = %machine_id, error = %err, "heartbeat touch failed");
            }
        }
        GatewayEvent::MachineEvent {
            machine_id,
            event,
            payload,
        } => handle_machine_event(state, &machine_id, &event, payload).await,
    }
}

async fn on_machine_connected(
    state: &AppState,
    machine_id: String,
    hello: agent_fleet_protocol::connection::HelloPayload,
    connection_id: String,
) {
    // Best-effort snapshot queries. A daemon that answers slowly still
    // registers; the dashboard fills in providers/workspaces later.
    let timeout = Duration::from_millis(state.config.machines.request_timeout_ms.clamp(500, 5_000));

    let providers = match state
        .connections
        .request_with_timeout(&machine_id, methods::PROVIDERS_LIST, json!({}), timeout)
        .await
    {
        Ok(value) => match serde_json::from_value::<Vec<ProviderInfo>>(value) {
            Ok(providers) => providers,
            Err(err) => {
                tracing::warn!(machine = %machine_id, error = %err, "invalid provider snapshot");
                Vec::new()
            }
        },
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
        Ok(value) => match serde_json::from_value::<Vec<Workspace>>(value) {
            Ok(workspaces) => workspaces,
            Err(err) => {
                tracing::warn!(machine = %machine_id, error = %err, "invalid workspace snapshot");
                Vec::new()
            }
        },
        Err(err) => {
            tracing::debug!(machine = %machine_id, code = %err.code, "workspace snapshot unavailable");
            Vec::new()
        }
    };

    for workspace in &workspaces {
        if let Err(err) = state.db.upsert_workspace(workspace).await {
            tracing::warn!(
                machine = %machine_id,
                workspace = %workspace.id,
                error = %err,
                "failed to persist workspace snapshot"
            );
        }
    }

    let summaries: Vec<WorkspaceSummary> = workspaces.iter().map(WorkspaceSummary::from).collect();

    let registration = MachineRegistration {
        machine_id: machine_id.clone(),
        name: hello.name,
        hostname: hello.hostname,
        os: hello.os,
        arch: hello.arch,
        daemon_version: hello.daemon_version,
        protocol_version: hello.protocol_version,
        public_key: hello.public_key,
        providers,
        workspaces: summaries,
    };

    if let Err(err) = state
        .db
        .ensure_machine_org(&machine_id, agent_fleet_protocol::authz::DEFAULT_ORG_ID)
        .await
    {
        tracing::warn!(machine = %machine_id, error = %err, "failed to assign default organization");
    }

    let machine = match state.machines.register_or_update(registration).await {
        Ok(machine) => {
            if let Err(err) = state.machines.set_online(&machine_id).await {
                tracing::warn!(machine = %machine_id, error = %err, "failed to mark machine online");
            }
            Some(machine)
        }
        Err(err) => {
            tracing::warn!(machine = %machine_id, error = %err, "machine registration failed");
            state.machines.get(&machine_id).await.ok().flatten()
        }
    };

    let payload = machine
        .map(|machine| serde_json::to_value(machine).unwrap_or(Value::Null))
        .unwrap_or_else(|| json!({"id": machine_id, "status": "online"}));
    let mut status_event = PlatformEvent::new(connection_events::MACHINE_STATUS, payload);
    status_event.machine_id = Some(machine_id.clone());
    state.events.publish(status_event);

    let mut connection_event = PlatformEvent::new(
        connection_events::CONNECTION_STATE,
        json!({
            "machine_id": machine_id,
            "connection_id": connection_id,
            "state": "online",
        }),
    );
    connection_event.machine_id = Some(machine_id);
    state.events.publish(connection_event);
}

async fn on_machine_disconnected(state: &AppState, machine_id: &str) {
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
    payload: Value,
) {
    let mut platform = base_platform_event(machine_id, event_name, payload.clone());

    match event_name {
        connection_events::AGENT_EVENT => match serde_json::from_value::<AgentEvent>(payload) {
            Ok(agent_event) => {
                platform.session_id = Some(agent_event.session_id.clone());
                platform.sequence = state.event_index.next_sequence(machine_id);
                apply_agent_event(state, machine_id, &agent_event).await;
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

    state.events.publish(platform);
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
                if let Err(err) = state.db.upsert_session(&session).await {
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

            if let Err(err) = state.db.upsert_permission(&permission).await {
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
    match state.db.get_session(session_id).await {
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
            if let Err(err) = state.db.upsert_session(&session).await {
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

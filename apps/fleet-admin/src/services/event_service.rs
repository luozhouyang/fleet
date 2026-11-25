//! Machine-event pipeline: gateway events in, [`PlatformEvent`]s out.
//!
//! The task is deliberately forgiving: a malformed event is logged and
//! skipped, never allowed to terminate the consumer.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use fleet_admin_storage::transaction::AdminTransaction;
use fleet_core_connection::GatewayEvent;
use fleet_core_protocol::agent::{
    AgentEvent, AgentEventType, AgentPermissionRequest, AgentQuestionRequest, AgentSession,
    ProviderInfo, SessionErrorPayload, SessionStatus, StatusChangedPayload,
};
use fleet_core_protocol::connection::{capabilities, events as connection_events, PlatformEvent};
use fleet_core_protocol::machine::MachineRegistration;
use fleet_core_protocol::rpc::{methods, AckEventsRequest};
use fleet_core_protocol::workspace::{Workspace, WorkspaceEvent, WorkspaceSummary};
use serde_json::{json, Value};

use crate::state::AppState;

/// In-memory indexes maintained while consuming machine events.
///
/// They complement the database: the session-to-machine cache lets decision
/// routes resolve the owning daemon without a SQLite read for every event.
#[derive(Clone)]
pub struct EventIndexes {
    sequences: Arc<DashMap<String, AtomicU64>>,
    machine_orgs: Arc<DashMap<String, String>>,
    /// Session -> machine cache so high-rate streaming events do not hit
    /// SQLite for every delta.
    session_machines: Arc<DashMap<String, String>>,
    /// Machines revoked during this process, so the hot event path avoids a
    /// SQLite read for every streaming delta.
    revoked_machines: Arc<DashMap<String, ()>>,
    seen_agent_events: Arc<Mutex<SeenEvents>>,
    /// Applied durable cursors waiting to be confirmed in one batch ack.
    pending_acks: Arc<DashMap<String, Mutex<BTreeSet<u64>>>>,
    /// Cursors deferred because the admin cannot resolve their session yet.
    /// Entries expire after a bound so one unknown session cannot stall a
    /// machine's contiguous cursor forever.
    unresolved_cursors: Arc<DashMap<String, Mutex<HashMap<u64, Instant>>>>,
    /// Last session-list reconciliation attempt per machine.
    session_syncs: Arc<DashMap<String, Instant>>,
    /// Last time a heartbeat was persisted per machine.
    heartbeat_persists: Arc<DashMap<String, Instant>>,
}

/// How long an unknown-session event may hold the cursor before it is dropped.
const UNRESOLVED_EVENT_TIMEOUT: Duration = Duration::from_secs(60);
/// Minimum interval between session-list reconciliations for one machine.
const SESSION_SYNC_INTERVAL: Duration = Duration::from_secs(5);
/// Persist a machine heartbeat at most once per interval.
const HEARTBEAT_PERSIST_INTERVAL: Duration = Duration::from_secs(60);
/// Applied cursors confirmed in one `events.ack` request.
const ACK_FLUSH_BATCH: usize = 512;

struct SeenEvents {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

const SEEN_AGENT_EVENT_LIMIT: usize = 32_768;

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
            machine_orgs: Arc::new(DashMap::new()),
            session_machines: Arc::new(DashMap::new()),
            revoked_machines: Arc::new(DashMap::new()),
            seen_agent_events: Arc::new(Mutex::new(SeenEvents {
                ids: HashSet::new(),
                order: VecDeque::new(),
            })),
            pending_acks: Arc::new(DashMap::new()),
            unresolved_cursors: Arc::new(DashMap::new()),
            session_syncs: Arc::new(DashMap::new()),
            heartbeat_persists: Arc::new(DashMap::new()),
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

    /// Cache the organization that owns a machine.
    pub fn set_machine_org(&self, machine_id: &str, org_id: &str) {
        self.machine_orgs
            .insert(machine_id.to_string(), org_id.to_string());
    }

    /// Forget a cached machine owner, so events fail closed until a claim.
    pub fn clear_machine_org(&self, machine_id: &str) {
        self.machine_orgs.remove(machine_id);
    }

    /// Cached machine for a session.
    pub fn session_machine(&self, session_id: &str) -> Option<String> {
        self.session_machines
            .get(session_id)
            .map(|entry| entry.value().clone())
    }

    /// Cache which machine owns a session.
    pub fn set_session_machine(&self, session_id: &str, machine_id: &str) {
        self.session_machines
            .insert(session_id.to_string(), machine_id.to_string());
    }

    /// Whether the cached owner matches, without cloning the id.
    pub fn session_machine_matches(&self, session_id: &str, machine_id: &str) -> Option<bool> {
        self.session_machines
            .get(session_id)
            .map(|entry| entry.value() == machine_id)
    }

    /// Forget a deleted session.
    pub fn remove_session_machine(&self, session_id: &str) {
        self.session_machines.remove(session_id);
    }

    /// Forget every cached session owned by a removed machine.
    pub fn remove_sessions_for_machine(&self, machine_id: &str) {
        self.session_machines.retain(|_, owner| owner != machine_id);
    }

    /// Remember a permanent machine revocation for the hot event path.
    pub fn mark_machine_revoked(&self, machine_id: &str) {
        self.revoked_machines.insert(machine_id.to_string(), ());
    }

    /// Forget a revocation after an explicit unrevoke.
    pub fn clear_machine_revoked(&self, machine_id: &str) {
        self.revoked_machines.remove(machine_id);
    }

    /// Whether this process has revoked the machine.
    pub fn is_machine_revoked(&self, machine_id: &str) -> bool {
        self.revoked_machines.contains_key(machine_id)
    }

    /// Whether a durable agent event was already applied in this process.
    pub fn is_agent_event_processed(&self, event_id: &str) -> bool {
        let seen = self
            .seen_agent_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        seen.ids.contains(event_id)
    }

    /// Forget the in-process event cache (tests simulate a restart).
    #[cfg(test)]
    fn clear_agent_events(&self) {
        let mut seen = self
            .seen_agent_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        seen.ids.clear();
        seen.order.clear();
    }

    /// Record a durable agent event id. Returns `false` when the event was
    /// already applied (for example a replay after an unacknowledged write).
    ///
    /// Callers must only mark an event after its database projection was
    /// applied successfully: an unapplied id in this set would turn a replay
    /// into a duplicate that skips the projection.
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

    /// Queue an applied durable cursor for the next batch acknowledgement.
    pub fn note_ack(&self, machine_id: &str, cursor: u64) {
        let queue = self
            .pending_acks
            .entry(machine_id.to_string())
            .or_insert_with(|| Mutex::new(BTreeSet::new()));
        let Ok(mut queue) = queue.lock() else {
            return;
        };
        queue.insert(cursor);
    }

    /// Drain up to one acknowledgement batch for a machine.
    pub fn drain_acks(&self, machine_id: &str, limit: usize) -> Vec<u64> {
        let Some(queue) = self.pending_acks.get(machine_id) else {
            return Vec::new();
        };
        let Ok(mut queue) = queue.lock() else {
            return Vec::new();
        };
        let mut drained = Vec::with_capacity(limit.min(queue.len()));
        while drained.len() < limit {
            let Some(sequence) = queue.pop_first() else {
                break;
            };
            drained.push(sequence);
        }
        drained
    }

    /// Machines holding unconfirmed acknowledgements.
    pub fn machines_with_pending_acks(&self) -> Vec<String> {
        self.pending_acks
            .iter()
            .filter(|entry| entry.value().lock().map(|q| !q.is_empty()).unwrap_or(false))
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Remember a cursor whose session cannot be resolved yet.
    pub fn note_unresolved(&self, machine_id: &str, cursor: u64) {
        let entry = self
            .unresolved_cursors
            .entry(machine_id.to_string())
            .or_insert_with(|| Mutex::new(HashMap::new()));
        let Ok(mut cursors) = entry.lock() else {
            return;
        };
        cursors.entry(cursor).or_insert_with(Instant::now);
    }

    /// Clear a cursor once its event was applied, acknowledged or dropped.
    pub fn resolve_unresolved(&self, machine_id: &str, cursor: u64) {
        let Some(entry) = self.unresolved_cursors.get(machine_id) else {
            return;
        };
        let Ok(mut cursors) = entry.lock() else {
            return;
        };
        cursors.remove(&cursor);
    }

    /// Cursors that have held the machine cursor past the timeout.
    pub fn expire_unresolved(&self, timeout: Duration) -> Vec<(String, Vec<u64>)> {
        let now = Instant::now();
        let mut expired = Vec::new();
        for entry in self.unresolved_cursors.iter() {
            let machine_id = entry.key().clone();
            let Ok(mut cursors) = entry.value().lock() else {
                continue;
            };
            let mut due: Vec<u64> = cursors
                .iter()
                .filter(|(_, first_seen)| now.duration_since(**first_seen) >= timeout)
                .map(|(cursor, _)| *cursor)
                .collect();
            due.sort_unstable();
            for cursor in &due {
                cursors.remove(cursor);
            }
            if !due.is_empty() {
                expired.push((machine_id, due));
            }
        }
        expired
    }

    /// Start a session reconciliation unless one ran recently.
    pub fn begin_session_sync(&self, machine_id: &str, interval: Duration) -> bool {
        let now = Instant::now();
        if let Some(previous) = self.session_syncs.get(machine_id) {
            if now.duration_since(*previous) < interval {
                return false;
            }
        }
        self.session_syncs.insert(machine_id.to_string(), now);
        true
    }

    /// Allow the next call to `begin_session_sync` to run immediately.
    pub fn force_session_sync(&self, machine_id: &str) {
        self.session_syncs.remove(machine_id);
    }

    /// Whether the heartbeat for this machine should be persisted now.
    pub fn heartbeat_persist_due(&self, machine_id: &str, interval: Duration) -> bool {
        let now = Instant::now();
        if let Some(previous) = self.heartbeat_persists.get(machine_id) {
            if now.duration_since(*previous) < interval {
                return false;
            }
        }
        self.heartbeat_persists.insert(machine_id.to_string(), now);
        true
    }
}

/// Spawn the gateway-event consumer. The task runs until the gateway closes.
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.gateway.subscribe();
    let maintenance_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let maintenance_state = state.clone();
    let maintenance_shutdown = Arc::clone(&maintenance_stop);
    tokio::spawn(async move {
        maintenance_loop(maintenance_state, maintenance_shutdown).await;
    });
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => handle_gateway_event(&state, event).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "machine event consumer lagged");
                    // Control events (connect/disconnect/heartbeat) share this
                    // broadcast and have no replay path. Reconcile the machine
                    // registry after a gap instead of trusting a partial view.
                    resync_gateway_state(&state).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        maintenance_stop.store(true, Ordering::Relaxed);
        tracing::debug!("machine event consumer stopped");
    })
}

/// Periodic work that must not block the event consumer.
async fn maintenance_loop(state: AppState, shutdown: Arc<std::sync::atomic::AtomicBool>) {
    let mut acks = tokio::time::interval(Duration::from_millis(100));
    acks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sweeps = tokio::time::interval(Duration::from_secs(5));
    sweeps.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut resyncs = tokio::time::interval(Duration::from_secs(60));
    resyncs.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            _ = acks.tick() => flush_ack_batches(&state).await,
            _ = sweeps.tick() => expire_unresolved_events(&state).await,
            _ = resyncs.tick() => resync_gateway_state(&state).await,
        }
    }
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
            // Keep the hot path in memory; persisting every heartbeat is a
            // SQLite write per machine that competes with event projection.
            if state
                .event_index
                .heartbeat_persist_due(&machine_id, HEARTBEAT_PERSIST_INTERVAL)
            {
                if let Err(err) = state.machines.touch(&machine_id).await {
                    tracing::debug!(machine = %machine_id, error = %err, "heartbeat touch failed");
                }
            } else if !state.machines.touch_cached(&machine_id) {
                if let Err(err) = state.machines.touch(&machine_id).await {
                    tracing::debug!(machine = %machine_id, error = %err, "heartbeat touch failed");
                }
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

    // Reconcile sessions before relying on event ownership. If the admin lost
    // its session rows (database restore) the daemon's replay would otherwise
    // defer every dependent event for this machine.
    state.event_index.force_session_sync(&machine_id);
    let sync_state = state.clone();
    let sync_machine = machine_id.clone();
    tokio::spawn(async move {
        sync_machine_sessions(&sync_state, &sync_machine).await;
    });

    let state = state.clone();
    tokio::spawn(async move {
        refresh_machine_snapshot(state, machine_id, hello, connection_id).await;
    });
}

/// Reconcile persisted session rows with a connected daemon.
///
/// The session list is the authoritative set of live and historical sessions
/// on a machine. Upserting it gives the event pipeline an ownership row for
/// events that arrive before (or without) a replayed `session.created`.
async fn sync_machine_sessions(state: &AppState, machine_id: &str) {
    if !state
        .event_index
        .begin_session_sync(machine_id, SESSION_SYNC_INTERVAL)
    {
        return;
    }
    let timeout = Duration::from_millis(state.config.machines.request_timeout_ms.clamp(500, 5_000));
    let value = match state
        .connections
        .request_with_timeout(machine_id, methods::SESSIONS_LIST, json!({}), timeout)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            tracing::debug!(machine = %machine_id, code = %err.code, "session reconciliation unavailable");
            return;
        }
    };
    let sessions = match serde_json::from_value::<Vec<AgentSession>>(value) {
        Ok(sessions) => sessions,
        Err(err) => {
            tracing::warn!(machine = %machine_id, error = %err, "invalid session list during reconciliation");
            return;
        }
    };
    for session in sessions {
        if session.machine_id != machine_id {
            tracing::warn!(
                machine = %machine_id,
                session = %session.id,
                owner = %session.machine_id,
                "ignoring a session from another machine during reconciliation"
            );
            continue;
        }
        state
            .event_index
            .set_session_machine(&session.id, &session.machine_id);
        if let Err(err) = state.db.sessions.upsert(&session).await {
            tracing::warn!(
                machine = %machine_id,
                session = %session.id,
                error = %err,
                "failed to persist a reconciled session"
            );
        }
    }
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

    let org_id = machine_org_for_event(state, machine_id).await;
    let payload = machine
        .map(|machine| serde_json::to_value(machine).unwrap_or(Value::Null))
        .unwrap_or_else(|| json!({"id": machine_id, "status": "online"}));
    let mut status_event = PlatformEvent::new(connection_events::MACHINE_STATUS, payload);
    status_event.machine_id = Some(machine_id.to_string());
    status_event.org_id = org_id.clone();
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
    connection_event.org_id = org_id;
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
    mark_machine_offline(state, machine_id).await;
}

/// Project a machine disconnect into the machine and session tables.
async fn mark_machine_offline(state: &AppState, machine_id: &str) {
    let payload = match state.machines.set_offline(machine_id).await {
        Ok(machine) => serde_json::to_value(machine).unwrap_or(Value::Null),
        Err(err) => {
            tracing::warn!(machine = %machine_id, error = %err, "failed to mark machine offline");
            json!({"id": machine_id, "status": "offline"})
        }
    };
    let mut event = PlatformEvent::new(connection_events::MACHINE_STATUS, payload);
    event.machine_id = Some(machine_id.to_string());
    event.org_id = machine_org_for_event(state, machine_id).await;
    state.events.publish(event);
    interrupt_active_sessions(state, machine_id).await;
}

/// A machine that is offline cannot still be running a provider.
///
/// The session list reconciliation also applies this rule when it is queried;
/// doing it on disconnect keeps the REST projection and the event stream from
/// disagreeing while nobody is looking at the list.
async fn interrupt_active_sessions(state: &AppState, machine_id: &str) {
    let sessions = match state.db.sessions.list(Some(machine_id)).await {
        Ok(sessions) => sessions,
        Err(err) => {
            tracing::warn!(machine = %machine_id, error = %err, "failed to read sessions for disconnect");
            return;
        }
    };
    for mut session in sessions {
        if !session.status.is_active() {
            continue;
        }
        session.status = SessionStatus::Interrupted;
        session.last_error = Some(fleet_core_common::ApiError::new(
            "machine_offline",
            "the machine is offline; this session is read-only until it reconnects",
        ));
        if !session.metadata.is_object() {
            session.metadata = json!({});
        }
        session.metadata["machine_offline"] = json!(true);
        session.updated_at = fleet_core_common::now();
        if let Err(err) = state.db.sessions.upsert(&session).await {
            tracing::warn!(
                machine = %machine_id,
                session = %session.id,
                error = %err,
                "failed to interrupt an offline session"
            );
        }
    }
}

/// Reconcile the machine registry after a gateway broadcast gap.
///
/// Connection lifecycle events have no replay path, so a lagging consumer can
/// miss a connect or disconnect. Compare the live registry with the persisted
/// snapshot and repair both directions; durable agent events recover through
/// the daemon outbox independently.
async fn resync_gateway_state(state: &AppState) {
    state.metrics.record_gateway_resync();
    let online: HashSet<String> = state
        .connections
        .online()
        .into_iter()
        .map(|info| info.machine_id)
        .collect();
    let machines = match state.machines.list().await {
        Ok(machines) => machines,
        Err(err) => {
            tracing::warn!(error = %err, "failed to list machines for gateway resync");
            return;
        }
    };
    for machine in machines {
        let connected = online.contains(&machine.id);
        if connected && !machine.status.is_online() {
            match state.machines.set_online(&machine.id).await {
                Ok(machine) => {
                    if let Ok(Some(org_id)) = state.db.machine_orgs.get(&machine.id).await {
                        state.event_index.set_machine_org(&machine.id, &org_id);
                    }
                    let payload = serde_json::to_value(&machine).unwrap_or(Value::Null);
                    let mut event = PlatformEvent::new(connection_events::MACHINE_STATUS, payload);
                    event.machine_id = Some(machine.id.clone());
                    event.org_id = machine_org_for_event(state, &machine.id).await;
                    state.events.publish(event);
                }
                Err(err) => {
                    tracing::warn!(machine = %machine.id, error = %err, "gateway resync failed to mark machine online");
                }
            }
            state.event_index.force_session_sync(&machine.id);
            let sync_state = state.clone();
            let sync_machine = machine.id.clone();
            tokio::spawn(async move {
                sync_machine_sessions(&sync_state, &sync_machine).await;
            });
        } else if !connected && machine.status.is_online() {
            tracing::warn!(
                machine = %machine.id,
                "gateway resync found a machine without a live connection"
            );
            mark_machine_offline(state, &machine.id).await;
        }
    }
}

async fn handle_machine_event(
    state: &AppState,
    machine_id: &str,
    event_name: &str,
    sequence: u64,
    cursor: Option<u64>,
    payload: Value,
) {
    // An event queued just before a revocation must not repopulate metadata.
    // The revocation route updates this cache; avoid a SQLite read per event.
    if state.event_index.is_machine_revoked(machine_id) {
        tracing::debug!(machine = %machine_id, "dropping event for a revoked machine");
        return;
    }
    let mut platform = base_platform_event(machine_id, event_name, payload.clone());
    platform.org_id = machine_org_for_event(state, machine_id).await;
    platform.sequence = if sequence > 0 {
        sequence
    } else {
        state.event_index.next_sequence(machine_id)
    };
    let mut duplicate = false;
    let mut acknowledge = true;

    match event_name {
        connection_events::AGENT_EVENT => match serde_json::from_value::<AgentEvent>(payload) {
            Ok(agent_event) => {
                platform.session_id = Some(agent_event.session_id.clone());
                match agent_event_ownership(state, machine_id, &agent_event).await {
                    EventOwnership::Owned => {
                        if let Some(cursor) = cursor {
                            state.event_index.resolve_unresolved(machine_id, cursor);
                        }
                        // Only durable frames are replayed and projected by
                        // the admin. Ephemeral deltas are live-only.
                        if cursor.is_some() {
                            if state
                                .event_index
                                .is_agent_event_processed(&agent_event.event_id)
                            {
                                duplicate = true;
                            } else {
                                match apply_durable_agent_event(state, machine_id, &agent_event)
                                    .await
                                {
                                    Ok(ApplyOutcome::Applied) => {
                                        state.event_index.mark_agent_event(&agent_event.event_id);
                                    }
                                    Ok(ApplyOutcome::Duplicate) => {
                                        // Applied before an admin restart.
                                        state.event_index.mark_agent_event(&agent_event.event_id);
                                        duplicate = true;
                                    }
                                    Err(err) => {
                                        tracing::warn!(
                                            machine = %machine_id,
                                            session = %agent_event.session_id,
                                            event_id = %agent_event.event_id,
                                            error = %err,
                                            "failed to apply agent event; withholding acknowledgement"
                                        );
                                        // Do not publish state that was not
                                        // persisted, and do not acknowledge.
                                        duplicate = true;
                                        acknowledge = false;
                                    }
                                }
                            }
                        }
                    }
                    EventOwnership::Foreign => {
                        if let Some(cursor) = cursor {
                            state.event_index.resolve_unresolved(machine_id, cursor);
                        }
                        // The connection is authenticated for `machine_id`; a
                        // daemon must not mutate another machine's sessions
                        // just by naming them in an event payload.
                        tracing::warn!(
                            machine = %machine_id,
                            session = %agent_event.session_id,
                            event_id = %agent_event.event_id,
                            "dropping agent event whose session is not owned by this machine"
                        );
                        duplicate = true;
                    }
                    EventOwnership::Unknown => {
                        // Session metadata is missing (for example after an
                        // admin database restore). Reconcile with the daemon
                        // and let replay retry; the unresolved timer bounds the
                        // head-of-line block if the session never appears.
                        tracing::debug!(
                            machine = %machine_id,
                            session = %agent_event.session_id,
                            event_id = %agent_event.event_id,
                            "deferring agent event for an unknown session"
                        );
                        if let Some(cursor) = cursor {
                            state.event_index.note_unresolved(machine_id, cursor);
                        }
                        let sync_state = state.clone();
                        let sync_machine = machine_id.to_string();
                        tokio::spawn(async move {
                            sync_machine_sessions(&sync_state, &sync_machine).await;
                        });
                        duplicate = true;
                        acknowledge = false;
                    }
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
        if acknowledge {
            acknowledge_daemon_event(state, machine_id, cursor);
        }
    }
}

/// Confirm delivery of a durable daemon event. Acknowledgement failures are
/// harmless: the event stays in the outbox and is replayed later.
///
/// Daemons that support `ack_batch` accumulate applied cursors and confirm
/// them in one request; older daemons keep the exact-sequence request.
fn acknowledge_daemon_event(state: &AppState, machine_id: &str, cursor: u64) {
    if state
        .connections
        .supports(machine_id, capabilities::ACK_BATCH)
    {
        state.event_index.note_ack(machine_id, cursor);
        return;
    }
    acknowledge_single(state, machine_id, cursor);
}

/// Confirm a set of applied cursors, batching when the daemon supports it.
fn acknowledge_daemon_cursors(state: &AppState, machine_id: &str, cursors: &[u64]) {
    if state
        .connections
        .supports(machine_id, capabilities::ACK_BATCH)
    {
        for cursor in cursors {
            state.event_index.note_ack(machine_id, *cursor);
        }
        return;
    }
    let state = state.clone();
    let machine_id = machine_id.to_string();
    let cursors = cursors.to_vec();
    tokio::spawn(async move {
        for cursor in cursors {
            if let Err(error) = send_single_ack(&state, &machine_id, cursor).await {
                tracing::debug!(
                    machine = %machine_id,
                    cursor,
                    error = %error,
                    "failed to acknowledge daemon event"
                );
            }
        }
    });
}

fn acknowledge_single(state: &AppState, machine_id: &str, cursor: u64) {
    let state = state.clone();
    let machine_id = machine_id.to_string();
    tokio::spawn(async move {
        if let Err(error) = send_single_ack(&state, &machine_id, cursor).await {
            tracing::debug!(
                machine = %machine_id,
                cursor,
                error = %error,
                "failed to acknowledge daemon event"
            );
        }
    });
}

async fn send_single_ack(
    state: &AppState,
    machine_id: &str,
    cursor: u64,
) -> std::result::Result<(), fleet_core_common::ApiError> {
    let payload = serde_json::to_value(AckEventsRequest {
        cursor,
        cursors: Vec::new(),
    })
    .unwrap_or(Value::Null);
    state
        .connections
        .request(machine_id, methods::EVENTS_ACK, payload)
        .await
        .map(|_| ())
}

/// Send one batch acknowledgement per machine.
async fn flush_ack_batches(state: &AppState) {
    for machine_id in state.event_index.machines_with_pending_acks() {
        let cursors = state.event_index.drain_acks(&machine_id, ACK_FLUSH_BATCH);
        if cursors.is_empty() {
            continue;
        }
        let cursor = cursors.last().copied().unwrap_or_default();
        state.metrics.record_ack_batch(cursors.len());
        let payload =
            serde_json::to_value(AckEventsRequest { cursor, cursors }).unwrap_or(Value::Null);
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = state
                .connections
                .request(&machine_id, methods::EVENTS_ACK, payload)
                .await
            {
                tracing::debug!(
                    machine = %machine_id,
                    error = %error,
                    "failed to acknowledge a batch of daemon events"
                );
            }
        });
    }
}

/// Drop deferred events whose session never became resolvable.
///
/// The alternative is a permanent head-of-line block for the whole machine.
/// Acknowledgement is the only way to move the daemon cursor past the event;
/// the drop is logged so an operator can investigate the missing session.
async fn expire_unresolved_events(state: &AppState) {
    for (machine_id, cursors) in state
        .event_index
        .expire_unresolved(UNRESOLVED_EVENT_TIMEOUT)
    {
        tracing::warn!(
            machine = %machine_id,
            count = cursors.len(),
            "dropping durable events whose session never became resolvable"
        );
        state.metrics.record_unresolved_events(cursors.len());
        acknowledge_daemon_cursors(state, &machine_id, &cursors);
    }
}

/// Build the platform envelope for a daemon event.
fn base_platform_event(machine_id: &str, event_name: &str, payload: Value) -> PlatformEvent {
    let mut event = PlatformEvent::new(event_name, payload);
    event.machine_id = Some(machine_id.to_string());
    event
}

/// Resolve and cache the organization that owns a machine for event scoping.
pub(crate) async fn machine_org_for_event(state: &AppState, machine_id: &str) -> Option<String> {
    if let Some(org_id) = state.event_index.machine_org(machine_id) {
        return Some(org_id);
    }
    match state.db.machine_orgs.get(machine_id).await {
        Ok(org_id) => {
            if let Some(org_id) = &org_id {
                state.event_index.set_machine_org(machine_id, org_id);
            }
            org_id
        }
        Err(error) => {
            tracing::warn!(machine = %machine_id, error = %error, "failed to resolve machine organization");
            None
        }
    }
}

enum EventOwnership {
    Owned,
    Unknown,
    Foreign,
}

/// Classify an agent event against the connection's machine.
///
/// `SessionCreated` carries the full session and is checked against its
/// payload. `SessionDeleted` for an already-removed row is an idempotent no-op.
/// Other events for unknown sessions are deferred (not acknowledged) so an
/// out-of-order replay can deliver `session.created` first.
fn cached_session_ownership(
    state: &AppState,
    machine_id: &str,
    session_id: &str,
) -> Option<EventOwnership> {
    state
        .event_index
        .session_machine_matches(session_id, machine_id)
        .map(|matches| {
            if matches {
                EventOwnership::Owned
            } else {
                EventOwnership::Foreign
            }
        })
}

/// Look up and cache a session's stored owner. Missing sessions are `Unknown`.
async fn stored_session_ownership(
    state: &AppState,
    machine_id: &str,
    session_id: &str,
) -> EventOwnership {
    match state.db.sessions.get(session_id).await {
        Ok(Some(session)) => {
            state
                .event_index
                .set_session_machine(session_id, &session.machine_id);
            if session.machine_id == machine_id {
                EventOwnership::Owned
            } else {
                EventOwnership::Foreign
            }
        }
        Ok(None) => EventOwnership::Unknown,
        Err(error) => {
            tracing::warn!(
                session = %session_id,
                error = %error,
                "failed to verify session ownership"
            );
            EventOwnership::Unknown
        }
    }
}

async fn agent_event_ownership(
    state: &AppState,
    machine_id: &str,
    event: &AgentEvent,
) -> EventOwnership {
    // `SessionCreated` carries ownership in its payload, so it must be
    // validated before consulting the session cache.
    if event.event_type == AgentEventType::SessionCreated {
        return match parse_agent_session(&event.payload) {
            Some(session) if session.id == event.session_id && session.machine_id == machine_id => {
                if let Some(cached) = cached_session_ownership(state, machine_id, &event.session_id)
                {
                    return cached;
                }
                // An existing row owned by another machine must not be
                // overwritten by a recycled session id.
                match state.db.sessions.get(&session.id).await {
                    Ok(Some(existing)) if existing.machine_id != machine_id => {
                        EventOwnership::Foreign
                    }
                    Ok(_) => {
                        state
                            .event_index
                            .set_session_machine(&session.id, machine_id);
                        EventOwnership::Owned
                    }
                    Err(error) => {
                        tracing::warn!(
                            session = %event.session_id,
                            error = %error,
                            "failed to verify session ownership"
                        );
                        EventOwnership::Unknown
                    }
                }
            }
            // A creation whose payload cannot be parsed is not proof of
            // foreign ownership. Defer it like any unknown session so the
            // bounded reconciliation window can repair the local view.
            None => EventOwnership::Unknown,
            // A parsed payload that disagrees with the envelope (session id
            // or machine) is forged or misrouted and must be dropped.
            Some(_) => EventOwnership::Foreign,
        };
    }

    // Permission events name their session in the payload. Validate that
    // before the cache shortcut: otherwise a warm cache would skip the check
    // and accept a permission event describing another session.
    if matches!(
        event.event_type,
        AgentEventType::PermissionRequested
            | AgentEventType::PermissionApproved
            | AgentEventType::PermissionRejected
    ) {
        let session_matches = parse_permission(&event.payload)
            .is_some_and(|permission| permission.session_id == event.session_id);
        if !session_matches {
            return EventOwnership::Foreign;
        }
    }

    if matches!(
        event.event_type,
        AgentEventType::QuestionRequested
            | AgentEventType::QuestionAnswered
            | AgentEventType::QuestionRejected
    ) {
        let session_matches = parse_question(&event.payload)
            .is_some_and(|question| question.session_id == event.session_id);
        if !session_matches {
            return EventOwnership::Foreign;
        }
    }

    if let Some(ownership) = cached_session_ownership(state, machine_id, &event.session_id) {
        return ownership;
    }
    match event.event_type {
        AgentEventType::SessionDeleted => {
            match stored_session_ownership(state, machine_id, &event.session_id).await {
                EventOwnership::Unknown => EventOwnership::Owned,
                ownership => ownership,
            }
        }
        _ => stored_session_ownership(state, machine_id, &event.session_id).await,
    }
}

/// Outcome of applying one durable agent event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyOutcome {
    /// The projection and the processed id committed.
    Applied,
    /// The durable table already recorded this event; nothing was applied.
    Duplicate,
}

/// Apply one durable agent event and record its metric outcome.
async fn apply_durable_agent_event(
    state: &AppState,
    machine_id: &str,
    event: &AgentEvent,
) -> Result<ApplyOutcome, String> {
    let outcome = apply_durable_agent_event_inner(state, machine_id, event).await;
    let metric = match &outcome {
        Ok(ApplyOutcome::Applied) => crate::metrics::DurableOutcome::Applied,
        Ok(ApplyOutcome::Duplicate) => crate::metrics::DurableOutcome::Duplicate,
        Err(_) => crate::metrics::DurableOutcome::Failed,
    };
    state.metrics.record_durable(metric);
    outcome
}

/// Apply one durable agent event inside a service-owned transaction.
///
/// The service owns the scope: it begins the transaction, applies the
/// projection, records the processed event id and commits. A projection or
/// mark failure rolls both writes back and leaves the event unacknowledged,
/// so the daemon replays it.
async fn apply_durable_agent_event_inner(
    state: &AppState,
    machine_id: &str,
    event: &AgentEvent,
) -> Result<ApplyOutcome, String> {
    let mut tx = state
        .db
        .begin_transaction()
        .await
        .map_err(|err| format!("failed to begin the event transaction: {err}"))?;
    // A replay after an admin restart finds the id here; the in-memory cache
    // is only a fast path and does not survive the restart.
    let already_processed = tx
        .processed_events()
        .contains(machine_id, &event.event_id)
        .await
        .map_err(|err| format!("failed to read the processed-event table: {err}"))?;
    if already_processed {
        // Dropping the transaction rolls back the read-only scope.
        return Ok(ApplyOutcome::Duplicate);
    }

    project_agent_event(&mut tx, event).await?;
    tx.processed_events()
        .mark(machine_id, &event.event_id)
        .await
        .map_err(|err| format!("failed to persist the processed event id: {err}"))?;
    tx.commit()
        .await
        .map_err(|err| format!("failed to commit the event transaction: {err}"))?;

    // Post-commit cache maintenance and retention. The cache must never be
    // updated before the database commit succeeds.
    if event.event_type == AgentEventType::SessionDeleted {
        state.event_index.remove_session_machine(&event.session_id);
    }
    if let Err(err) = state.db.processed_events.prune_if_due(machine_id).await {
        tracing::warn!(
            machine = %machine_id,
            error = %err,
            "failed to prune processed event ids"
        );
    }
    Ok(ApplyOutcome::Applied)
}

/// Apply the admin-side projection for one durable agent event.
///
/// This is business logic over transaction-scoped table operations; it never
/// commits. The service decides the transaction scope.
async fn project_agent_event(tx: &mut AdminTransaction, event: &AgentEvent) -> Result<(), String> {
    match event.event_type {
        AgentEventType::SessionCreated => {
            if let Some(session) = parse_agent_session(&event.payload) {
                // A replayed creation must not overwrite newer state that was
                // already applied (the sequence gap model can replay an older
                // event after a newer one).
                let existing = tx
                    .sessions()
                    .get(&session.id)
                    .await
                    .map_err(|err| format!("failed to read session: {err}"))?;
                let newer = existing
                    .as_ref()
                    .map(|existing| existing.updated_at <= session.updated_at)
                    .unwrap_or(true);
                if newer {
                    tx.sessions()
                        .upsert(&session)
                        .await
                        .map_err(|err| format!("failed to upsert session: {err}"))?;
                }
            } else {
                patch_session(
                    tx,
                    &event.session_id,
                    None,
                    title_from_payload(&event.payload),
                    event.timestamp,
                )
                .await?;
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
            tx.sessions()
                .delete(&event.session_id)
                .await
                .map_err(|err| format!("failed to delete session after daemon event: {err}"))?;
        }
        AgentEventType::AgentStatusChanged => {
            patch_session(
                tx,
                &event.session_id,
                parse_status_changed(&event.payload),
                title_from_payload(&event.payload),
                event.timestamp,
            )
            .await?;
        }
        AgentEventType::SessionStarted | AgentEventType::SessionResumed => {
            patch_session(
                tx,
                &event.session_id,
                Some(SessionStatus::Running),
                title_from_payload(&event.payload),
                event.timestamp,
            )
            .await?;
        }
        AgentEventType::SessionTerminated => {
            patch_session(
                tx,
                &event.session_id,
                Some(SessionStatus::Terminated),
                title_from_payload(&event.payload),
                event.timestamp,
            )
            .await?;
        }
        AgentEventType::SessionError => {
            if let Ok(payload) =
                serde_json::from_value::<SessionErrorPayload>(event.payload.clone())
            {
                let existing = tx
                    .sessions()
                    .get(&event.session_id)
                    .await
                    .map_err(|err| format!("failed to read session: {err}"))?;
                if let Some(mut session) = existing {
                    if event.timestamp < session.updated_at {
                        return Ok(());
                    }
                    session.last_error = Some(payload.error);
                    if session.status.can_transition_to(SessionStatus::Failed) {
                        session.status = SessionStatus::Failed;
                    }
                    session.updated_at = event.timestamp;
                    tx.sessions()
                        .upsert(&session)
                        .await
                        .map_err(|err| format!("failed to persist session error: {err}"))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn patch_session(
    tx: &mut AdminTransaction,
    session_id: &str,
    status: Option<SessionStatus>,
    title: Option<String>,
    at: fleet_core_common::Timestamp,
) -> Result<(), String> {
    let existing = tx
        .sessions()
        .get(session_id)
        .await
        .map_err(|err| format!("failed to read session: {err}"))?;
    match existing {
        Some(mut session) => {
            // Out-of-order replay can deliver an older status after a newer
            // one. Do not move the projection backwards.
            if at < session.updated_at {
                tracing::debug!(
                    session = %session_id,
                    "ignoring stale session patch from a replay"
                );
                return Ok(());
            }
            if let Some(status) = status {
                session.status = status;
            }
            if let Some(title) = title {
                if !title.is_empty() {
                    session.title = Some(title);
                }
            }
            session.updated_at = at;
            tx.sessions()
                .upsert(&session)
                .await
                .map_err(|err| format!("failed to update session: {err}"))?;
        }
        None => tracing::debug!(
            session = %session_id,
            "event for a session that has no local row yet"
        ),
    }
    Ok(())
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

fn parse_question(value: &Value) -> Option<AgentQuestionRequest> {
    serde_json::from_value::<AgentQuestionRequest>(value.clone())
        .ok()
        .or_else(|| {
            value.get("question").and_then(|inner| {
                serde_json::from_value::<AgentQuestionRequest>(inner.clone()).ok()
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
    use fleet_core_common::now;
    use fleet_core_protocol::agent::{PermissionEventPayload, PermissionStatus};

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

    async fn seed_machine(state: &AppState, machine_id: &str) {
        let timestamp = now();
        state
            .db
            .machines
            .upsert(&fleet_core_protocol::machine::Machine {
                id: machine_id.into(),
                name: machine_id.into(),
                hostname: String::new(),
                os: String::new(),
                arch: String::new(),
                daemon_version: String::new(),
                protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
                status: fleet_core_protocol::machine::MachineStatus::Offline,
                created_at: timestamp,
                updated_at: timestamp,
                connected_at: None,
                last_seen_at: None,
                providers: Vec::new(),
                workspaces: Vec::new(),
                workspace_count: 0,
                public_key: None,
            })
            .await
            .unwrap();
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
    async fn disconnect_interrupts_active_sessions() {
        let state = state_with_online_machine("m_offline").await;
        let mut session =
            fleet_core_protocol::agent::AgentSession::new("m_offline", "mock", "mock", "/tmp");
        session.status = SessionStatus::Running;
        state.db.sessions.upsert(&session).await.unwrap();

        on_machine_disconnected(&state, "m_offline").await;

        let stored = state.db.sessions.get(&session.id).await.unwrap().unwrap();
        assert_eq!(stored.status, SessionStatus::Interrupted);
        assert_eq!(stored.metadata["machine_offline"], serde_json::json!(true));
        assert_eq!(
            stored.last_error.as_ref().map(|error| error.code.as_str()),
            Some("machine_offline")
        );
    }

    #[test]
    fn applied_cursors_drain_in_batches() {
        let indexes = EventIndexes::new();
        for cursor in 1..=600 {
            indexes.note_ack("m_1", cursor);
        }
        indexes.note_ack("m_2", 7);

        let first = indexes.drain_acks("m_1", 512);
        assert_eq!(first.len(), 512);
        assert_eq!(first.first(), Some(&1));
        assert_eq!(first.last(), Some(&512));
        assert_eq!(
            indexes.drain_acks("m_1", 512),
            (513..=600).collect::<Vec<_>>()
        );
        assert!(indexes.drain_acks("m_1", 512).is_empty());

        assert_eq!(
            indexes.machines_with_pending_acks(),
            vec!["m_2".to_string()]
        );
    }

    #[test]
    fn unresolved_cursors_expire_after_the_timeout() {
        let indexes = EventIndexes::new();
        indexes.note_unresolved("m_1", 3);
        indexes.note_unresolved("m_1", 4);
        indexes.note_unresolved("m_2", 9);

        assert!(indexes
            .expire_unresolved(Duration::from_secs(60))
            .is_empty());

        let mut expired = indexes.expire_unresolved(Duration::ZERO);
        expired.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            expired,
            vec![
                ("m_1".to_string(), vec![3, 4]),
                ("m_2".to_string(), vec![9]),
            ]
        );
        assert!(indexes.expire_unresolved(Duration::ZERO).is_empty());
    }

    #[test]
    fn session_reconciliation_is_debounced() {
        let indexes = EventIndexes::new();
        assert!(indexes.begin_session_sync("m_1", Duration::from_secs(5)));
        assert!(!indexes.begin_session_sync("m_1", Duration::from_secs(5)));
        indexes.force_session_sync("m_1");
        assert!(indexes.begin_session_sync("m_1", Duration::from_secs(5)));
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

        assert_eq!(
            apply_durable_agent_event(&state, "m_mirror", &event)
                .await
                .expect("apply event"),
            ApplyOutcome::Applied
        );

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
    fn session_machine_cache_routes_decisions() {
        let indexes = EventIndexes::new();
        indexes.set_session_machine("session_1", "machine_1");
        assert_eq!(
            indexes.session_machine("session_1"),
            Some("machine_1".to_string())
        );
        assert_eq!(indexes.session_machine("session_2"), None);
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

    #[tokio::test]
    async fn events_are_scoped_to_the_connected_machine() {
        let state = state_with_online_machine("machine_1").await;
        seed_machine(&state, "machine_2").await;
        let mut session = AgentSession::new("machine_1", "mock", "mock-agent", "/tmp");
        session.id = "session_owned".into();
        state.db.sessions.upsert(&session).await.unwrap();

        let child = AgentEvent::new(&session.id, 1, AgentEventType::AgentStatusChanged, "mock")
            .with_payload(&StatusChangedPayload {
                status: SessionStatus::Completed,
                reason: None,
            });
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &child).await,
            EventOwnership::Owned
        ));
        assert!(matches!(
            agent_event_ownership(&state, "machine_2", &child).await,
            EventOwnership::Foreign
        ));
        // The child above populated the cache. A forged creation for the same
        // cached id must still be rejected from its payload.
        let mut forged = session.clone();
        forged.machine_id = "machine_2".into();
        let forged = AgentEvent::new(&session.id, 4, AgentEventType::SessionCreated, "mock")
            .with_payload(&json!({ "session": forged }));
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &forged).await,
            EventOwnership::Foreign
        ));

        let unknown = AgentEvent::new("session_unknown", 2, AgentEventType::TurnStarted, "mock")
            .with_payload(&json!({}));
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &unknown).await,
            EventOwnership::Unknown
        ));

        // A creation snapshot that does not deserialize is deferred rather
        // than treated as foreign: it may be an older/newer payload that the
        // session reconciliation can still repair.
        let malformed = AgentEvent::new(
            "session_malformed",
            7,
            AgentEventType::SessionCreated,
            "mock",
        )
        .with_payload(&json!({ "session": { "id": "session_malformed" } }));
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &malformed).await,
            EventOwnership::Unknown
        ));

        let mut foreign_session = session.clone();
        foreign_session.id = "session_foreign".into();
        foreign_session.machine_id = "machine_2".into();
        let created = AgentEvent::new(
            &foreign_session.id,
            3,
            AgentEventType::SessionCreated,
            "mock",
        )
        .with_payload(&json!({ "session": foreign_session }));
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &created).await,
            EventOwnership::Foreign
        ));

        let mut collision = foreign_session.clone();
        collision.id = "session_collision".into();
        state.db.sessions.upsert(&collision).await.unwrap();
        let mut recycled = collision.clone();
        recycled.machine_id = "machine_1".into();
        let created = AgentEvent::new(&recycled.id, 5, AgentEventType::SessionCreated, "mock")
            .with_payload(&json!({ "session": recycled }));
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &created).await,
            EventOwnership::Foreign
        ));

        let mismatched_permission = AgentEvent::new(
            "session_owned",
            6,
            AgentEventType::PermissionRequested,
            "mock",
        )
        .with_payload(&PermissionEventPayload {
            permission: AgentPermissionRequest {
                id: "perm_mismatch".into(),
                session_id: "session_other".into(),
                turn_id: None,
                tool_call_id: None,
                tool_name: "Bash".into(),
                description: "run".into(),
                input: json!({}),
                options: Vec::new(),
                status: PermissionStatus::Pending,
                created_at: now(),
                resolved_at: None,
                expires_at: None,
            },
        });
        // The cache is warm for `session_owned`; the payload check must still
        // reject a permission that names another session.
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &mismatched_permission).await,
            EventOwnership::Foreign
        ));
        assert_eq!(
            apply_durable_agent_event(&state, "machine_1", &mismatched_permission)
                .await
                .expect("apply event"),
            ApplyOutcome::Applied
        );

        handle_machine_event(
            &state,
            "machine_1",
            connection_events::AGENT_EVENT,
            4,
            Some(4),
            serde_json::to_value(&created).unwrap(),
        )
        .await;
        assert!(state
            .db
            .sessions
            .get("session_foreign")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn replayed_older_status_does_not_move_a_session_backwards() {
        let state = state_with_online_machine("machine_1").await;
        let mut session = AgentSession::new("machine_1", "mock", "mock-agent", "/tmp");
        session.id = "session_stale".into();
        session.status = SessionStatus::Completed;
        session.updated_at = now();
        state.db.sessions.upsert(&session).await.unwrap();

        let mut event = AgentEvent::new(&session.id, 1, AgentEventType::AgentStatusChanged, "mock")
            .with_payload(&StatusChangedPayload {
                status: SessionStatus::Running,
                reason: None,
            });
        event.timestamp = now() - chrono::Duration::seconds(30);
        assert_eq!(
            apply_durable_agent_event(&state, "machine_1", &event)
                .await
                .expect("apply event"),
            ApplyOutcome::Applied
        );

        let stored = state.db.sessions.get(&session.id).await.unwrap().unwrap();
        assert_eq!(stored.status, SessionStatus::Completed);
    }

    #[tokio::test]
    async fn question_event_with_foreign_session_is_rejected() {
        let state = state_with_online_machine("machine_1").await;
        let mut event = AgentEvent::new("session_1", 1, AgentEventType::QuestionRequested, "mock")
            .with_payload(&json!({
                "question": {
                    "id": "question_1",
                    "session_id": "session_other",
                    "questions": [],
                    "status": "pending",
                    "created_at": "2026-11-17T00:00:00Z"
                }
            }));
        event.timestamp = now();
        assert!(matches!(
            agent_event_ownership(&state, "machine_1", &event).await,
            EventOwnership::Foreign
        ));
    }

    #[tokio::test]
    async fn failed_projection_is_neither_marked_nor_acknowledged() {
        let state = state_with_online_machine("machine_1").await;
        let mut session = AgentSession::new("machine_1", "mock", "mock-agent", "/tmp");
        session.id = "session_ack".into();
        state.db.sessions.upsert(&session).await.unwrap();
        state
            .event_index
            .set_session_machine(&session.id, "machine_1");

        let event = AgentEvent::new(&session.id, 1, AgentEventType::AgentStatusChanged, "mock")
            .with_payload(&StatusChangedPayload {
                status: SessionStatus::Running,
                reason: None,
            });
        // Simulate the admin database becoming unavailable after the event
        // arrived. Closing the pool makes every later statement fail.
        state.db.pool().close().await;

        handle_machine_event(
            &state,
            "machine_1",
            connection_events::AGENT_EVENT,
            1,
            Some(1),
            serde_json::to_value(&event).unwrap(),
        )
        .await;

        // The daemon cursor must not be acknowledged, so the event stays in
        // the outbox and is replayed once the database recovers. The dedup
        // mark must also stay unset so the replay applies the projection.
        assert!(!state.event_index.is_agent_event_processed(&event.event_id));
        let (metrics, _) = state.metrics.render();
        assert!(
            metrics.contains("fleet_admin_durable_events_total{outcome=\"failed\"} 1"),
            "failure must increment the failed outcome:\n{metrics}"
        );
    }

    #[tokio::test]
    async fn persisted_event_ids_suppress_replays_after_a_cache_reset() {
        let state = state_with_online_machine("machine_1").await;
        let mut session = AgentSession::new("machine_1", "mock", "mock-agent", "/tmp");
        session.id = "session_persisted".into();
        state.db.sessions.upsert(&session).await.unwrap();
        // Keep ownership resolvable even after the row is deleted below.
        state
            .event_index
            .set_session_machine(&session.id, "machine_1");

        let event = AgentEvent::new(&session.id, 1, AgentEventType::SessionCreated, "mock")
            .with_payload(&json!({ "session": session }));
        let payload = serde_json::to_value(&event).unwrap();
        handle_machine_event(
            &state,
            "machine_1",
            connection_events::AGENT_EVENT,
            1,
            Some(1),
            payload.clone(),
        )
        .await;
        assert!(state.db.sessions.get(&session.id).await.unwrap().is_some());
        assert!(state
            .db
            .processed_events
            .contains("machine_1", &event.event_id)
            .await
            .unwrap());

        // Simulate an admin restart: the in-memory cache is empty, but the
        // durable dedup row survives.
        state.event_index.clear_agent_events();
        // A second projection would recreate this row.
        state.db.sessions.delete(&session.id).await.unwrap();
        handle_machine_event(
            &state,
            "machine_1",
            connection_events::AGENT_EVENT,
            1,
            Some(1),
            payload,
        )
        .await;

        assert!(
            state.db.sessions.get(&session.id).await.unwrap().is_none(),
            "a persisted processed id must suppress re-application after a restart"
        );
        let (metrics, _) = state.metrics.render();
        assert!(
            metrics.contains("fleet_admin_durable_events_total{outcome=\"applied\"} 1"),
            "the first delivery must increment applied:\n{metrics}"
        );
        assert!(
            metrics.contains("fleet_admin_durable_events_total{outcome=\"duplicate\"} 1"),
            "the replay must increment duplicate:\n{metrics}"
        );
    }

    #[tokio::test]
    async fn successful_projection_is_marked_for_duplicate_suppression() {
        let state = state_with_online_machine("machine_1").await;
        let mut session = AgentSession::new("machine_1", "mock", "mock-agent", "/tmp");
        session.id = "session_ok".into();
        state.db.sessions.upsert(&session).await.unwrap();
        state
            .event_index
            .set_session_machine(&session.id, "machine_1");

        let event = AgentEvent::new(&session.id, 1, AgentEventType::AgentStatusChanged, "mock")
            .with_payload(&StatusChangedPayload {
                status: SessionStatus::Running,
                reason: None,
            });
        handle_machine_event(
            &state,
            "machine_1",
            connection_events::AGENT_EVENT,
            1,
            Some(1),
            serde_json::to_value(&event).unwrap(),
        )
        .await;

        assert!(state.event_index.is_agent_event_processed(&event.event_id));
        let stored = state.db.sessions.get(&session.id).await.unwrap().unwrap();
        assert_eq!(stored.status, SessionStatus::Running);
    }
}

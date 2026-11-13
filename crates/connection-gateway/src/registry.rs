use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_fleet_common::{error_codes, now, ApiError, MachineId, Timestamp};
use agent_fleet_protocol::connection::{ConnectionEnvelope, DisconnectPayload};
use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Live connection information exposed to the control plane API.
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub machine_id: MachineId,
    pub connection_id: String,
    pub connected_at: Timestamp,
    pub last_seen_at: Timestamp,
}

pub(crate) struct ConnectionHandle {
    pub machine_id: MachineId,
    pub connection_id: String,
    pub connected_at: Timestamp,
    pub sender: mpsc::Sender<ConnectionEnvelope>,
    pub pending: DashMap<String, oneshot::Sender<std::result::Result<serde_json::Value, ApiError>>>,
    pub last_seen_millis: AtomicI64,
    pub shutdown: CancellationToken,
    pub closed: AtomicBool,
}

impl ConnectionHandle {
    fn last_seen(&self) -> Timestamp {
        let millis = self.last_seen_millis.load(Ordering::Relaxed);
        chrono::DateTime::from_timestamp_millis(millis).unwrap_or_else(now)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// Registry of live machine connections.
///
/// This is intentionally in-memory: connection sockets are process-local state.
/// Persistent machine metadata lives in `agent-fleet-machine-registry`.
#[derive(Clone)]
pub struct ConnectionRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    connections: DashMap<MachineId, Arc<ConnectionHandle>>,
    request_timeout: Duration,
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::with_request_timeout(Duration::from_secs(30))
    }

    pub fn with_request_timeout(request_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                connections: DashMap::new(),
                request_timeout,
            }),
        }
    }

    pub fn is_online(&self, machine_id: &str) -> bool {
        self.inner
            .connections
            .get(machine_id)
            .map(|handle| !handle.is_closed())
            .unwrap_or(false)
    }

    pub fn online(&self) -> Vec<ConnectionInfo> {
        self.inner
            .connections
            .iter()
            .filter(|entry| !entry.is_closed())
            .map(|entry| ConnectionInfo {
                machine_id: entry.machine_id.clone(),
                connection_id: entry.connection_id.clone(),
                connected_at: entry.connected_at,
                last_seen_at: entry.last_seen(),
            })
            .collect()
    }

    pub fn connection_count(&self) -> usize {
        self.inner.connections.len()
    }

    pub fn last_seen(&self, machine_id: &str) -> Option<Timestamp> {
        self.inner
            .connections
            .get(machine_id)
            .map(|handle| handle.last_seen())
    }

    /// Send a request to a machine and await its correlated response.
    pub async fn request(
        &self,
        machine_id: &str,
        method: &str,
        payload: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, ApiError> {
        self.request_with_timeout(machine_id, method, payload, self.inner.request_timeout)
            .await
    }

    pub async fn request_with_timeout(
        &self,
        machine_id: &str,
        method: &str,
        payload: serde_json::Value,
        timeout: Duration,
    ) -> std::result::Result<serde_json::Value, ApiError> {
        let handle = self
            .inner
            .connections
            .get(machine_id)
            .map(|entry| entry.clone())
            .ok_or_else(|| {
                ApiError::new(
                    error_codes::MACHINE_OFFLINE,
                    format!("machine {machine_id} is not connected"),
                )
            })?;

        if handle.is_closed() {
            return Err(ApiError::new(
                error_codes::MACHINE_OFFLINE,
                format!("machine {machine_id} connection is closed"),
            ));
        }

        let envelope = ConnectionEnvelope::request_on(format!("rpc/{method}"), method, payload);
        let request_id = envelope
            .message_id
            .clone()
            .ok_or_else(|| ApiError::internal("request envelope missing message id"))?;

        let (tx, rx) = oneshot::channel();
        handle.pending.insert(request_id.clone(), tx);

        if handle.sender.send(envelope).await.is_err() {
            handle.pending.remove(&request_id);
            return Err(ApiError::new(
                error_codes::MACHINE_OFFLINE,
                "machine connection writer is gone",
            ));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ApiError::new(
                error_codes::MACHINE_OFFLINE,
                "machine disconnected before responding",
            )),
            Err(_) => {
                handle.pending.remove(&request_id);
                Err(ApiError::new(
                    error_codes::TIMEOUT,
                    format!("request `{method}` to machine {machine_id} timed out"),
                ))
            }
        }
    }

    /// Deliver a fire-and-forget event to a machine.
    pub async fn notify(
        &self,
        machine_id: &str,
        event: &str,
        payload: serde_json::Value,
    ) -> std::result::Result<(), ApiError> {
        let handle = self
            .inner
            .connections
            .get(machine_id)
            .map(|entry| entry.clone())
            .ok_or_else(|| {
                ApiError::new(
                    error_codes::MACHINE_OFFLINE,
                    format!("machine {machine_id} is not connected"),
                )
            })?;
        handle
            .sender
            .send(ConnectionEnvelope::event(event, payload))
            .await
            .map_err(|_| {
                ApiError::new(
                    error_codes::MACHINE_OFFLINE,
                    "machine connection writer is gone",
                )
            })
    }

    /// Ask a machine to disconnect and drop its connection.
    pub fn disconnect(&self, machine_id: &str) {
        if let Some((_, handle)) = self.inner.connections.remove(machine_id) {
            handle.closed.store(true, Ordering::Relaxed);
            let _ = handle.sender.try_send(ConnectionEnvelope::event(
                "connection.disconnect",
                serde_json::to_value(DisconnectPayload {
                    reason: Some("disconnected by control plane".into()),
                })
                .unwrap_or(serde_json::Value::Null),
            ));
            handle.shutdown.cancel();
            fail_pending(&handle, "machine disconnected by control plane");
        }
    }

    pub(crate) fn insert(&self, handle: Arc<ConnectionHandle>) {
        if let Some(old) = self
            .inner
            .connections
            .insert(handle.machine_id.clone(), handle)
        {
            old.closed.store(true, Ordering::Relaxed);
            old.shutdown.cancel();
            fail_pending(&old, "machine reconnected with a new connection");
        }
    }

    pub(crate) fn remove(&self, machine_id: &str, connection_id: &str) {
        let should_remove = self
            .inner
            .connections
            .get(machine_id)
            .map(|handle| handle.connection_id == connection_id)
            .unwrap_or(false);
        if should_remove {
            if let Some((_, handle)) = self.inner.connections.remove(machine_id) {
                handle.closed.store(true, Ordering::Relaxed);
                handle.shutdown.cancel();
                fail_pending(&handle, "machine connection closed");
            }
        }
    }
}

fn fail_pending(handle: &ConnectionHandle, message: &str) {
    let ids: Vec<String> = handle.pending.iter().map(|e| e.key().clone()).collect();
    for id in ids {
        if let Some((_, tx)) = handle.pending.remove(&id) {
            let _ = tx.send(Err(ApiError::new(
                error_codes::MACHINE_OFFLINE,
                message.to_string(),
            )));
        }
    }
}

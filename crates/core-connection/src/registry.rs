use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use fleet_core_common::{error_codes, now, ApiError, MachineId, Timestamp};
use fleet_core_protocol::connection::{ConnectionEnvelope, DisconnectPayload};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Live connection information exposed to the agent admin API.
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub machine_id: MachineId,
    pub connection_id: String,
    pub connected_at: Timestamp,
    pub last_seen_at: Timestamp,
}

type PendingSender = oneshot::Sender<std::result::Result<serde_json::Value, ApiError>>;
type PendingMap = DashMap<String, PendingSender>;

pub(crate) struct ConnectionHandle {
    pub machine_id: MachineId,
    pub connection_id: String,
    pub connected_at: Timestamp,
    pub sender: mpsc::Sender<ConnectionEnvelope>,
    pub pending: PendingMap,
    pub last_seen_millis: AtomicI64,
    pub shutdown: CancellationToken,
    pub closed: AtomicBool,
    /// Capabilities agreed during the handshake.
    pub capabilities: Vec<String>,
}

/// Releases a pending-request slot when the caller stops waiting.
///
/// The response path also removes the slot, so this is a no-op on success. It
/// matters when the request future is dropped (caller cancellation) or the
/// response timeout fires: without it the sender would stay registered until
/// the machine disconnects.
struct PendingRequestGuard<'a> {
    pending: &'a PendingMap,
    request_id: &'a str,
}

impl Drop for PendingRequestGuard<'_> {
    fn drop(&mut self) {
        self.pending.remove(self.request_id);
    }
}

impl ConnectionHandle {
    fn last_seen(&self) -> Timestamp {
        let millis = self.last_seen_millis.load(Ordering::Relaxed);
        chrono::DateTime::from_timestamp_millis(millis).unwrap_or_else(now)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn supports(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|advertised| advertised == capability)
    }
}

/// Registry of live machine connections.
///
/// This is intentionally in-memory: connection sockets are process-local state.
/// Persistent machine metadata lives in `fleet-admin-machine`.
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

    /// Connection id currently serving a machine, if any.
    pub fn current_connection_id(&self, machine_id: &str) -> Option<String> {
        self.inner
            .connections
            .get(machine_id)
            .map(|handle| handle.connection_id.clone())
    }

    pub fn is_online(&self, machine_id: &str) -> bool {
        self.inner
            .connections
            .get(machine_id)
            .map(|handle| !handle.is_closed())
            .unwrap_or(false)
    }

    /// Whether the live connection negotiated `capability`.
    ///
    /// Returns false when the machine is offline, so callers that need to
    /// distinguish "offline" from "too old" should check [`Self::is_online`]
    /// first.
    pub fn supports(&self, machine_id: &str, capability: &str) -> bool {
        self.inner
            .connections
            .get(machine_id)
            .map(|handle| !handle.is_closed() && handle.supports(capability))
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
        let _pending = PendingRequestGuard {
            pending: &handle.pending,
            request_id: &request_id,
        };

        match tokio::time::timeout(timeout, handle.sender.send(envelope)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(ApiError::new(
                    error_codes::MACHINE_OFFLINE,
                    "machine connection writer is gone",
                ));
            }
            Err(_) => {
                return Err(ApiError::new(
                    error_codes::TIMEOUT,
                    format!("machine {machine_id} outbound queue is full"),
                ));
            }
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ApiError::new(
                error_codes::MACHINE_OFFLINE,
                "machine disconnected before responding",
            )),
            Err(_) => Err(ApiError::new(
                error_codes::TIMEOUT,
                format!("request `{method}` to machine {machine_id} timed out"),
            )),
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
        let envelope = ConnectionEnvelope::event(event, payload);
        match tokio::time::timeout(self.inner.request_timeout, handle.sender.send(envelope)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(ApiError::new(
                error_codes::MACHINE_OFFLINE,
                "machine connection writer is gone",
            )),
            Err(_) => Err(ApiError::new(
                error_codes::TIMEOUT,
                "machine outbound queue is full",
            )),
        }
    }

    /// Ask a machine to disconnect and drop its connection.
    pub fn disconnect(&self, machine_id: &str) {
        if let Some((_, handle)) = self.inner.connections.remove(machine_id) {
            handle.closed.store(true, Ordering::Relaxed);
            let _ = handle.sender.try_send(ConnectionEnvelope::event(
                "connection.disconnect",
                serde_json::to_value(DisconnectPayload {
                    reason: Some("disconnected by agent admin".into()),
                })
                .unwrap_or(serde_json::Value::Null),
            ));
            handle.shutdown.cancel();
            fail_pending(&handle, "machine disconnected by agent admin");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(machine_id: &str) -> (Arc<ConnectionHandle>, mpsc::Receiver<ConnectionEnvelope>) {
        let (sender, receiver) = mpsc::channel(8);
        let handle = Arc::new(ConnectionHandle {
            machine_id: machine_id.to_string(),
            connection_id: "c1".to_string(),
            connected_at: now(),
            sender,
            pending: DashMap::new(),
            last_seen_millis: AtomicI64::new(now().timestamp_millis()),
            shutdown: CancellationToken::new(),
            closed: AtomicBool::new(false),
            capabilities: Vec::new(),
        });
        (handle, receiver)
    }

    #[tokio::test]
    async fn cancelled_requests_release_their_pending_slot() {
        let registry = ConnectionRegistry::with_request_timeout(Duration::from_secs(60));
        let (handle, _receiver) = handle("m1");
        registry.insert(handle.clone());

        let request_registry = registry.clone();
        let request = tokio::spawn(async move {
            request_registry
                .request("m1", "test.method", serde_json::json!({}))
                .await
        });
        for _ in 0..1_000 {
            if handle.pending.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(handle.pending.len(), 1);

        request.abort();
        assert!(request.await.is_err());
        assert!(
            handle.pending.is_empty(),
            "a cancelled request leaked its pending slot"
        );
    }
}

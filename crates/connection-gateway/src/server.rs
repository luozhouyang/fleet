use std::sync::Arc;
use std::time::Duration;

use agent_fleet_auth::{new_challenge, nonce_base64, verify_signature, TokenAuth};
use agent_fleet_common::{error_codes, now, ApiError, MachineId};
use agent_fleet_protocol::connection::{
    key_rotation_statement, AuthChallenge, AuthResponse, ConnectionAccepted, ConnectionEnvelope,
    HelloPayload, MessageType,
};
use agent_fleet_protocol::machine::{HeartbeatAckPayload, HeartbeatPayload};
use agent_fleet_protocol::rpc::methods;
use agent_fleet_protocol::PROTOCOL_VERSION;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::error::{GatewayError, Result};
use crate::registry::{ConnectionHandle, ConnectionRegistry};

/// Verifies the pinned public key for a machine id and pins the first key seen
/// (trust on first use, gated by the user bearer token on the upgrade request).
#[async_trait::async_trait]
pub trait MachineAuthenticator: Send + Sync {
    async fn pinned_key(&self, machine_id: &str) -> std::result::Result<Option<String>, ApiError>;
    async fn pin_key(
        &self,
        machine_id: &str,
        public_key: &str,
    ) -> std::result::Result<(), ApiError>;
}

/// Events emitted by the gateway for the control plane to consume.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    MachineConnected {
        machine_id: MachineId,
        hello: HelloPayload,
        connection_id: String,
    },
    MachineDisconnected {
        machine_id: MachineId,
        reason: Option<String>,
    },
    MachineEvent {
        machine_id: MachineId,
        event: String,
        payload: serde_json::Value,
    },
    Heartbeat {
        machine_id: MachineId,
        at: agent_fleet_common::Timestamp,
    },
}

pub struct ConnectionGateway {
    token_auth: TokenAuth,
    authenticator: Arc<dyn MachineAuthenticator>,
    registry: ConnectionRegistry,
    events: broadcast::Sender<GatewayEvent>,
    heartbeat_interval: Duration,
    heartbeat_timeout: Duration,
    handshake_timeout: Duration,
    outbound_capacity: usize,
}

impl ConnectionGateway {
    pub fn new(
        token_auth: TokenAuth,
        authenticator: Arc<dyn MachineAuthenticator>,
        heartbeat_interval: Duration,
        heartbeat_timeout: Duration,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            token_auth,
            authenticator,
            registry: ConnectionRegistry::with_request_timeout(Duration::from_secs(30)),
            events,
            heartbeat_interval,
            heartbeat_timeout,
            handshake_timeout: Duration::from_secs(10),
            outbound_capacity: 256,
        })
    }

    /// Override the per-request timeout used by [`ConnectionRegistry::request`].
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.registry = ConnectionRegistry::with_request_timeout(timeout);
        self
    }

    pub fn registry(&self) -> ConnectionRegistry {
        self.registry.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<GatewayEvent> {
        self.events.subscribe()
    }

    /// Axum router exposing `GET /machine` (mount it under `/ws`).
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/machine", get(machine_upgrade))
            .with_state(self)
    }

    async fn serve_socket(self: Arc<Self>, socket: WebSocket) {
        let (sink, mut stream) = socket.split();
        match self.handshake(sink, &mut stream).await {
            Ok((handle, hello)) => {
                self.run_connection(handle, hello, stream).await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "machine websocket handshake failed");
            }
        }
    }

    async fn handshake<S, R>(
        self: &Arc<Self>,
        mut sink: S,
        stream: &mut R,
    ) -> Result<(Arc<ConnectionHandle>, HelloPayload)>
    where
        S: futures::Sink<Message, Error = axum::Error> + Unpin + Send + 'static,
        R: futures::Stream<Item = std::result::Result<Message, axum::Error>> + Unpin,
    {
        // 1. hello
        let hello_env = read_envelope(stream, self.handshake_timeout).await?;
        if hello_env.message_type != MessageType::Request
            || hello_env.method.as_deref() != Some(methods::HELLO)
        {
            return Err(GatewayError::Protocol(
                "first frame must be connection.hello".into(),
            ));
        }
        let hello: HelloPayload = serde_json::from_value(hello_env.payload)?;
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(GatewayError::Protocol(format!(
                "unsupported protocol version {} (server speaks {})",
                hello.protocol_version, PROTOCOL_VERSION
            )));
        }
        if hello.machine_id.trim().is_empty() || hello.public_key.trim().is_empty() {
            return Err(GatewayError::Protocol(
                "machine_id and public_key are required".into(),
            ));
        }

        // 2. pin the first key (TOFU) or defer a changed key to the rotation
        // proof, which can only be validated after the challenge is issued.
        let pinned_key = self.authenticator.pinned_key(&hello.machine_id).await?;
        let rotation_required = match pinned_key.as_deref() {
            Some(pinned) if pinned != hello.public_key => true,
            Some(_) => false,
            None => {
                self.authenticator
                    .pin_key(&hello.machine_id, &hello.public_key)
                    .await?;
                false
            }
        };

        // 3. challenge
        let challenge = new_challenge();
        let challenge_payload = AuthChallenge {
            challenge_id: challenge.id.clone(),
            nonce: nonce_base64(&challenge),
            issued_at: now(),
            expires_at: Some(now() + chrono::Duration::seconds(60)),
        };
        sink.send(Message::text(
            ConnectionEnvelope::request(
                methods::AUTH_CHALLENGE,
                serde_json::to_value(&challenge_payload)?,
            )
            .to_json(),
        ))
        .await
        .map_err(|e| GatewayError::WebSocket(e.to_string()))?;

        // 4. auth response
        let auth_env = read_envelope(stream, self.handshake_timeout).await?;
        if auth_env.message_type != MessageType::Request
            || auth_env.method.as_deref() != Some(methods::AUTH_RESPONSE)
        {
            return Err(GatewayError::Protocol(
                "expected connection.auth response".into(),
            ));
        }
        let auth: AuthResponse = serde_json::from_value(auth_env.payload)?;
        if auth.machine_id != hello.machine_id {
            return Err(GatewayError::Auth("machine id mismatch".into()));
        }
        if auth.challenge_id != challenge.id {
            return Err(GatewayError::Auth("unknown challenge id".into()));
        }
        verify_signature(&hello.public_key, &challenge.nonce, &auth.signature)
            .map_err(|e| GatewayError::Auth(e.to_string()))?;

        // 5. accept a proof-of-possession key rotation signed by the old key.
        if rotation_required {
            let pinned = pinned_key
                .as_deref()
                .ok_or_else(|| GatewayError::Auth("missing pinned key for rotation".into()))?;
            let proof = auth.rotation.as_ref().ok_or_else(|| {
                GatewayError::Auth(format!(
                    "public key mismatch for machine {}; rotation proof required",
                    hello.machine_id
                ))
            })?;
            if proof.new_public_key != hello.public_key {
                return Err(GatewayError::Auth(
                    "rotation proof does not cover the presented key".into(),
                ));
            }
            let statement = key_rotation_statement(
                &hello.machine_id,
                &hello.public_key,
                &challenge_payload.nonce,
            );
            verify_signature(pinned, statement.as_bytes(), &proof.old_signature)
                .map_err(|e| GatewayError::Auth(format!("invalid rotation proof: {e}")))?;
            self.authenticator
                .pin_key(&hello.machine_id, &hello.public_key)
                .await?;
            tracing::info!(
                machine = %hello.machine_id,
                "machine public key rotated with a proof signed by the previous key"
            );
        }

        // 6. register the live connection
        let (outbound_tx, outbound_rx) = mpsc::channel(self.outbound_capacity);
        let connection_id = agent_fleet_common::new_id("conn");
        let handle = Arc::new(ConnectionHandle {
            machine_id: hello.machine_id.clone(),
            connection_id: connection_id.clone(),
            connected_at: now(),
            sender: outbound_tx,
            pending: dashmap::DashMap::new(),
            last_seen_millis: std::sync::atomic::AtomicI64::new(now().timestamp_millis()),
            shutdown: CancellationToken::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        // 6. accepted
        sink.send(Message::text(
            ConnectionEnvelope::request(
                methods::ACCEPTED,
                serde_json::to_value(ConnectionAccepted {
                    connection_id: connection_id.clone(),
                    machine_id: hello.machine_id.clone(),
                    server_version: env!("CARGO_PKG_VERSION").to_string(),
                    protocol_version: PROTOCOL_VERSION,
                    heartbeat_interval_ms: self.heartbeat_interval.as_millis() as u64,
                })?,
            )
            .to_json(),
        ))
        .await
        .map_err(|e| GatewayError::WebSocket(e.to_string()))?;

        // 7. register the connection and start the writer
        self.registry.insert(handle.clone());
        spawn_writer(sink, outbound_rx, handle.shutdown.clone());

        let _ = self.events.send(GatewayEvent::MachineConnected {
            machine_id: hello.machine_id.clone(),
            hello: hello.clone(),
            connection_id,
        });

        Ok((handle, hello))
    }

    async fn run_connection<R>(
        self: Arc<Self>,
        handle: Arc<ConnectionHandle>,
        hello: HelloPayload,
        mut stream: R,
    ) where
        R: futures::Stream<Item = std::result::Result<Message, axum::Error>> + Unpin,
    {
        let machine_id = handle.machine_id.clone();
        let reason: Option<String>;
        let mut monitor = tokio::time::interval(self.heartbeat_interval);
        monitor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = handle.shutdown.cancelled() => {
                    reason = Some("shutdown requested".into());
                    break;
                }
                _ = monitor.tick() => {
                    let last = handle.last_seen_millis.load(std::sync::atomic::Ordering::Relaxed);
                    let elapsed = now().timestamp_millis() - last;
                    if elapsed > self.heartbeat_timeout.as_millis() as i64 {
                        reason = Some("heartbeat timeout".into());
                        break;
                    }
                }
                message = stream.next() => {
                    match message {
                        None => {
                            reason = Some("socket closed".into());
                            break;
                        }
                        Some(Err(err)) => {
                            reason = Some(format!("websocket error: {err}"));
                            break;
                        }
                        Some(Ok(Message::Text(text))) => {
                            handle
                                .last_seen_millis
                                .store(now().timestamp_millis(), std::sync::atomic::Ordering::Relaxed);
                            let envelope = match ConnectionEnvelope::from_json(text.as_str()) {
                                Ok(envelope) => envelope,
                                Err(err) => {
                                    tracing::warn!(machine = %machine_id, error = %err, "invalid envelope");
                                    continue;
                                }
                            };
                            self.handle_envelope(&handle, envelope).await;
                        }
                        Some(Ok(Message::Binary(_))) => {
                            // Binary stream frames are not used in MVP.
                            tracing::debug!(machine = %machine_id, "ignoring binary frame");
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                            handle
                                .last_seen_millis
                                .store(now().timestamp_millis(), std::sync::atomic::Ordering::Relaxed);
                        }
                        Some(Ok(Message::Close(frame))) => {
                            reason = Some(match frame {
                                Some(frame) => frame.reason.to_string(),
                                None => "closed by peer".into(),
                            });
                            break;
                        }
                    }
                }
            }
        }

        handle
            .closed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        handle.shutdown.cancel();
        self.registry.remove(&machine_id, &handle.connection_id);
        let _ = self.events.send(GatewayEvent::MachineDisconnected {
            machine_id: machine_id.clone(),
            reason: reason.clone(),
        });
        let _ = self.events.send(GatewayEvent::MachineEvent {
            machine_id: machine_id.clone(),
            event: "connection.closed".into(),
            payload: serde_json::to_value(serde_json::json!({"reason": reason}))
                .unwrap_or_default(),
        });
        tracing::info!(machine = %machine_id, name = %hello.name, "machine disconnected");
    }

    async fn handle_envelope(&self, handle: &Arc<ConnectionHandle>, envelope: ConnectionEnvelope) {
        match envelope.message_type {
            MessageType::Response | MessageType::Error => {
                if let Some(request_id) = envelope.message_id.as_deref() {
                    if let Some((_, tx)) = handle.pending.remove(request_id) {
                        let result = if envelope.message_type == MessageType::Error {
                            Err(serde_json::from_value::<ApiError>(envelope.payload.clone())
                                .unwrap_or_else(|_| {
                                    ApiError::new(
                                        error_codes::PROTOCOL_ERROR,
                                        "machine returned a malformed error",
                                    )
                                }))
                        } else {
                            Ok(envelope.payload)
                        };
                        let _ = tx.send(result);
                    } else {
                        tracing::debug!(request_id, "response for unknown request id");
                    }
                }
            }
            MessageType::Event => {
                if let Some(event) = envelope.event {
                    let _ = self.events.send(GatewayEvent::MachineEvent {
                        machine_id: handle.machine_id.clone(),
                        event,
                        payload: envelope.payload,
                    });
                }
            }
            MessageType::Request => match envelope.method.as_deref() {
                Some(methods::HEARTBEAT) => {
                    let payload: HeartbeatPayload = serde_json::from_value(envelope.payload)
                        .unwrap_or(HeartbeatPayload {
                            sent_at: now(),
                            sequence: 0,
                            uptime_secs: 0,
                            active_sessions: 0,
                            status: None,
                        });
                    let _ = self.events.send(GatewayEvent::Heartbeat {
                        machine_id: handle.machine_id.clone(),
                        at: payload.sent_at,
                    });
                    let ack = HeartbeatAckPayload {
                        received_at: now(),
                        sequence: payload.sequence,
                        server_time: Some(now()),
                    };
                    let response = ConnectionEnvelope::response(
                        envelope.message_id.unwrap_or_default(),
                        serde_json::to_value(ack).unwrap_or_default(),
                    );
                    let _ = handle.sender.send(response).await;
                }
                other => {
                    let response = ConnectionEnvelope::error(
                        envelope.message_id,
                        ApiError::new(
                            error_codes::UNSUPPORTED,
                            format!(
                                "method `{}` is not supported by the control plane",
                                other.unwrap_or("<missing>")
                            ),
                        ),
                    );
                    let _ = handle.sender.send(response).await;
                }
            },
        }
    }
}

async fn read_envelope<R>(stream: &mut R, deadline: Duration) -> Result<ConnectionEnvelope>
where
    R: futures::Stream<Item = std::result::Result<Message, axum::Error>> + Unpin,
{
    let message = timeout(deadline, stream.next())
        .await
        .map_err(|_| GatewayError::Timeout("handshake timed out".into()))?
        .ok_or_else(|| GatewayError::Protocol("socket closed during handshake".into()))?
        .map_err(|err| GatewayError::WebSocket(err.to_string()))?;
    match message {
        Message::Text(text) => ConnectionEnvelope::from_json(text.as_str()).map_err(Into::into),
        Message::Close(_) => Err(GatewayError::Protocol(
            "socket closed during handshake".into(),
        )),
        other => Err(GatewayError::Protocol(format!(
            "expected text frame during handshake, got {other:?}"
        ))),
    }
}

fn spawn_writer<S>(
    mut sink: S,
    mut rx: mpsc::Receiver<ConnectionEnvelope>,
    shutdown: CancellationToken,
) where
    S: futures::Sink<Message, Error = axum::Error> + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                envelope = rx.recv() => {
                    match envelope {
                        Some(envelope) => {
                            if sink.send(Message::text(envelope.to_json())).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        let _ = sink.close().await;
    });
}

async fn machine_upgrade(
    State(gateway): State<Arc<ConnectionGateway>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if let Err(err) = gateway.token_auth.verify_bearer(auth) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(ApiError::new(error_codes::UNAUTHORIZED, err.to_string())),
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| async move { gateway.serve_socket(socket).await })
}

/// Helper for control-plane state: implements the authenticator over a closure.
pub struct FnAuthenticator<F, G> {
    pinned: F,
    pin: G,
}

impl<F, G> FnAuthenticator<F, G> {
    pub fn new(pinned: F, pin: G) -> Self {
        Self { pinned, pin }
    }
}

#[async_trait::async_trait]
impl<F, Fut, G, Fut2> MachineAuthenticator for FnAuthenticator<F, G>
where
    F: Fn(String) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = std::result::Result<Option<String>, ApiError>> + Send,
    G: Fn(String, String) -> Fut2 + Send + Sync,
    Fut2: std::future::Future<Output = std::result::Result<(), ApiError>> + Send,
{
    async fn pinned_key(&self, machine_id: &str) -> std::result::Result<Option<String>, ApiError> {
        (self.pinned)(machine_id.to_string()).await
    }

    async fn pin_key(
        &self,
        machine_id: &str,
        public_key: &str,
    ) -> std::result::Result<(), ApiError> {
        (self.pin)(machine_id.to_string(), public_key.to_string()).await
    }
}

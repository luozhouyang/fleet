use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fleet_core_auth::MachineIdentity;
use fleet_core_common::{error_codes, now, ApiError};
use fleet_core_protocol::connection::{
    key_rotation_statement, AuthChallenge, AuthResponse, ConnectionAccepted, ConnectionEnvelope,
    ConnectionState, HelloPayload, KeyRotationProof, MessageType,
};
use fleet_core_protocol::machine::HeartbeatPayload;
use fleet_core_protocol::rpc::methods;
use fleet_core_protocol::PROTOCOL_VERSION;
use futures::{SinkExt, StreamExt};
use rand::Rng;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::error::{GatewayError, Result};

/// Handles requests sent by the agent admin to this daemon.
#[async_trait::async_trait]
pub trait DaemonRequestHandler: Send + Sync {
    async fn handle(
        &self,
        method: &str,
        payload: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, ApiError>;

    async fn on_connected(&self, _accepted: &ConnectionAccepted) {}

    async fn on_disconnected(&self, _reason: &str) {}
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub server_url: String,
    pub token: String,
    pub machine_id: String,
    pub machine_name: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub heartbeat_interval: Duration,
    pub reconnect_initial: Duration,
    pub reconnect_max: Duration,
    pub handshake_timeout: Duration,
}

impl ClientConfig {
    pub fn new(
        server_url: impl Into<String>,
        token: impl Into<String>,
        machine_id: impl Into<String>,
        machine_name: impl Into<String>,
    ) -> Self {
        Self {
            server_url: server_url.into(),
            token: token.into(),
            machine_id: machine_id.into(),
            machine_name: machine_name.into(),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            heartbeat_interval: Duration::from_secs(15),
            reconnect_initial: Duration::from_millis(500),
            reconnect_max: Duration::from_secs(30),
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

struct ClientState {
    connection_id: RwLock<Option<String>>,
}

pub struct MachineConnectionClient {
    config: ClientConfig,
    identity: MachineIdentity,
    rotation_identity: Option<MachineIdentity>,
    handler: Arc<dyn DaemonRequestHandler>,
    state: AtomicU8,
    state_detail: ClientState,
    heartbeat_sequence: AtomicU64,
    event_sequence: AtomicU64,
    event_tx: broadcast::Sender<(String, serde_json::Value, Option<u64>)>,
}

impl MachineConnectionClient {
    pub fn new(
        config: ClientConfig,
        identity: MachineIdentity,
        handler: Arc<dyn DaemonRequestHandler>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            identity,
            rotation_identity: None,
            handler,
            state: AtomicU8::new(encode_state(ConnectionState::Disconnected)),
            state_detail: ClientState {
                connection_id: RwLock::new(None),
            },
            heartbeat_sequence: AtomicU64::new(0),
            event_sequence: AtomicU64::new(0),
            event_tx: broadcast::channel(1024).0,
        })
    }

    /// Attach the previous identity so the next handshake can prove a key
    /// rotation signed by the old key. `self.identity` must be the new key.
    pub fn with_rotation_identity(self: Arc<Self>, previous: MachineIdentity) -> Arc<Self> {
        Arc::new(Self {
            config: self.config.clone(),
            identity: self.identity.clone(),
            rotation_identity: Some(previous),
            handler: self.handler.clone(),
            state: AtomicU8::new(self.state.load(Ordering::Relaxed)),
            state_detail: ClientState {
                connection_id: RwLock::new(None),
            },
            heartbeat_sequence: AtomicU64::new(0),
            event_sequence: AtomicU64::new(self.event_sequence.load(Ordering::Relaxed)),
            event_tx: self.event_tx.clone(),
        })
    }

    /// Queue an event for the agent admin. Events emitted while disconnected
    /// are dropped; the agent admin reconciles state on reconnect.
    pub fn send_event(&self, event: impl Into<String>, payload: serde_json::Value) {
        let _ = self.event_tx.send((event.into(), payload, None));
    }

    /// Queue a durable event together with its outbox cursor. The admin can
    /// acknowledge the cursor once it has applied the event.
    pub fn send_event_with_cursor(
        &self,
        event: impl Into<String>,
        payload: serde_json::Value,
        cursor: u64,
    ) {
        let _ = self.event_tx.send((event.into(), payload, Some(cursor)));
    }

    pub fn state(&self) -> ConnectionState {
        decode_state(self.state.load(Ordering::Relaxed))
    }

    pub fn connection_id(&self) -> Option<String> {
        self.state_detail
            .connection_id
            .try_read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn set_state(&self, state: ConnectionState) {
        self.state.store(encode_state(state), Ordering::Relaxed);
    }

    /// Connect, authenticate, serve requests and reconnect until cancelled.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        let mut attempt: u32 = 0;
        loop {
            if shutdown.is_cancelled() {
                break;
            }
            self.set_state(ConnectionState::Connecting);
            let started = std::time::Instant::now();
            match self.connect_once(shutdown.clone()).await {
                Ok(()) => {
                    tracing::info!("machine connection closed");
                }
                Err(err @ GatewayError::Revoked(_)) => {
                    tracing::error!(error = %err, "machine was revoked by the admin; stopping");
                    break;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "machine connection failed");
                }
            }
            *self.state_detail.connection_id.write().await = None;
            self.handler.on_disconnected("connection closed").await;
            self.set_state(ConnectionState::Disconnected);

            if shutdown.is_cancelled() {
                break;
            }
            // A connection that stayed up for a while resets the backoff.
            if started.elapsed() > Duration::from_secs(30) {
                attempt = 0;
            }
            let delay = backoff(&self.config, attempt);
            attempt = attempt.saturating_add(1);
            tracing::info!(delay_ms = delay.as_millis() as u64, "reconnecting");
            self.set_state(ConnectionState::Reconnecting);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = shutdown.cancelled() => break,
            }
        }
        self.set_state(ConnectionState::Disconnected);
    }

    async fn connect_once(&self, shutdown: CancellationToken) -> Result<()> {
        let mut request = self
            .config
            .server_url
            .as_str()
            .into_client_request()
            .map_err(|err| GatewayError::Http(err.to_string()))?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.config.token)
                .parse()
                .map_err(|_| GatewayError::Unauthorized("invalid token header".into()))?,
        );

        let (socket, _response) = tokio::time::timeout(
            self.config.handshake_timeout,
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| GatewayError::Timeout("websocket connect timed out".into()))??;

        let (mut sink, mut stream) = socket.split();
        self.set_state(ConnectionState::Authenticating);

        // hello
        let hello = HelloPayload {
            machine_id: self.config.machine_id.clone(),
            name: self.config.machine_name.clone(),
            hostname: hostname(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            daemon_version: self.config.daemon_version.clone(),
            protocol_version: self.config.protocol_version,
            public_key: self.identity.public_key_base64(),
        };
        sink.send(Message::Text(
            ConnectionEnvelope::request(
                methods::HELLO,
                serde_json::to_value(&hello).map_err(GatewayError::from)?,
            )
            .to_json()
            .into(),
        ))
        .await?;

        // challenge
        let challenge_env = read_envelope(&mut stream, self.config.handshake_timeout).await?;
        if challenge_env.message_type == MessageType::Error {
            let err = serde_json::from_value::<ApiError>(challenge_env.payload)
                .unwrap_or_else(|_| ApiError::new(error_codes::UNAUTHORIZED, "rejected"));
            if err.code == error_codes::FORBIDDEN {
                return Err(GatewayError::Revoked(err.message));
            }
            return Err(GatewayError::Auth(err.to_string()));
        }
        if challenge_env.method.as_deref() != Some(methods::AUTH_CHALLENGE) {
            return Err(GatewayError::Protocol(
                "expected connection.auth_challenge".into(),
            ));
        }
        let challenge: AuthChallenge =
            serde_json::from_value(challenge_env.payload).map_err(GatewayError::from)?;
        let nonce =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &challenge.nonce)
                .map_err(|err| GatewayError::Auth(format!("invalid challenge nonce: {err}")))?;
        let rotation = self.rotation_identity.as_ref().map(|previous| {
            let new_public_key = self.identity.public_key_base64();
            let statement =
                key_rotation_statement(&self.config.machine_id, &new_public_key, &challenge.nonce);
            KeyRotationProof {
                new_public_key,
                old_signature: previous.sign(statement.as_bytes()),
            }
        });
        let auth = AuthResponse {
            machine_id: self.config.machine_id.clone(),
            challenge_id: challenge.challenge_id.clone(),
            signature: self.identity.sign(&nonce),
            rotation,
        };

        sink.send(Message::Text(
            ConnectionEnvelope::request(
                methods::AUTH_RESPONSE,
                serde_json::to_value(&auth).map_err(GatewayError::from)?,
            )
            .to_json()
            .into(),
        ))
        .await?;

        // accepted
        let accepted_env = read_envelope(&mut stream, self.config.handshake_timeout).await?;
        if accepted_env.message_type == MessageType::Error {
            let err = serde_json::from_value::<ApiError>(accepted_env.payload)
                .unwrap_or_else(|_| ApiError::new(error_codes::UNAUTHORIZED, "rejected"));
            if err.code == error_codes::FORBIDDEN {
                return Err(GatewayError::Revoked(err.message));
            }
            return Err(GatewayError::Auth(err.to_string()));
        }
        if accepted_env.method.as_deref() != Some(methods::ACCEPTED) {
            return Err(GatewayError::Protocol(
                "expected connection.accepted".into(),
            ));
        }
        let accepted: ConnectionAccepted =
            serde_json::from_value(accepted_env.payload).map_err(GatewayError::from)?;
        *self.state_detail.connection_id.write().await = Some(accepted.connection_id.clone());
        self.set_state(ConnectionState::Connected);
        tracing::info!(
            machine = %accepted.machine_id,
            connection = %accepted.connection_id,
            "machine connected"
        );
        self.handler.on_connected(&accepted).await;

        // writer task + heartbeat + stale-connection monitor
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<ConnectionEnvelope>(256);
        let local_shutdown = CancellationToken::new();
        let last_inbound = Arc::new(AtomicI64::new(now().timestamp_millis()));

        let writer_shutdown = local_shutdown.clone();
        let writer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = writer_shutdown.cancelled() => break,
                    envelope = outbound_rx.recv() => {
                        match envelope {
                            Some(envelope) => {
                                if sink.send(Message::Text(envelope.to_json().into())).await.is_err() {
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

        let heartbeat_shutdown = local_shutdown.clone();
        let heartbeat_tx = outbound_tx.clone();
        let heartbeat_interval = self.config.heartbeat_interval;
        let heartbeat_sequence = Arc::new(AtomicU64::new(
            self.heartbeat_sequence.load(Ordering::Relaxed),
        ));
        let heartbeat = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(heartbeat_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = heartbeat_shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        let seq = heartbeat_sequence.fetch_add(1, Ordering::Relaxed) + 1;
                        let payload = HeartbeatPayload {
                            sent_at: now(),
                            sequence: seq,
                            uptime_secs: 0,
                            active_sessions: 0,
                            status: None,
                        };
                        let envelope = ConnectionEnvelope::request(
                            methods::HEARTBEAT,
                            serde_json::to_value(payload).unwrap_or_default(),
                        );
                        if heartbeat_tx.send(envelope).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let forward_shutdown = local_shutdown.clone();
        let mut event_rx = self.event_tx.subscribe();
        let event_outbound = outbound_tx.clone();
        let event_sequence = Arc::new(AtomicU64::new(self.event_sequence.load(Ordering::Relaxed)));
        let forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = forward_shutdown.cancelled() => break,
                    event = event_rx.recv() => {
                        match event {
                            Ok((name, payload, cursor)) => {
                                let sequence = event_sequence.fetch_add(1, Ordering::Relaxed) + 1;
                                let mut envelope = ConnectionEnvelope::event(name, payload)
                                    .with_sequence(sequence);
                                if let Some(cursor) = cursor {
                                    envelope = envelope.with_cursor(cursor);
                                }
                                if event_outbound.send(envelope).await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(skipped, "event forwarder lagged");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });

        let monitor_shutdown = local_shutdown.clone();
        let monitor_last = last_inbound.clone();
        let stale_after = (self.config.heartbeat_interval * 3).max(Duration::from_secs(45));
        let monitor = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = monitor_shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        let last = monitor_last.load(Ordering::Relaxed);
                        if now().timestamp_millis() - last > stale_after.as_millis() as i64 {
                            tracing::warn!("no inbound machine traffic; closing connection");
                            monitor_shutdown.cancel();
                            break;
                        }
                    }
                }
            }
        });

        // read loop
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = local_shutdown.cancelled() => break,
                message = stream.next() => {
                    match message {
                        None => break,
                        Some(Err(err)) => {
                            tracing::warn!(error = %err, "websocket read error");
                            break;
                        }
                        Some(Ok(Message::Text(text))) => {
                            last_inbound.store(now().timestamp_millis(), Ordering::Relaxed);
                            let envelope = match ConnectionEnvelope::from_json(text.as_str()) {
                                Ok(envelope) => envelope,
                                Err(err) => {
                                    tracing::warn!(error = %err, "invalid envelope from agent admin");
                                    continue;
                                }
                            };
                            self.handle_incoming(envelope, &outbound_tx).await;
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                            last_inbound.store(now().timestamp_millis(), Ordering::Relaxed);
                        }
                        Some(Ok(Message::Close(_))) => break,
                        Some(Ok(Message::Binary(_))) => {}
                        Some(Ok(_)) => {}
                    }
                }
            }
        }

        local_shutdown.cancel();
        let _ = writer.await;
        heartbeat.abort();
        monitor.abort();
        forwarder.abort();
        self.set_state(ConnectionState::Disconnected);
        Ok(())
    }

    async fn handle_incoming(
        &self,
        envelope: ConnectionEnvelope,
        outbound: &mpsc::Sender<ConnectionEnvelope>,
    ) {
        match envelope.message_type {
            MessageType::Request => {
                let Some(method) = envelope.method.clone() else {
                    let _ = outbound
                        .send(ConnectionEnvelope::error(
                            envelope.message_id,
                            ApiError::invalid("missing method"),
                        ))
                        .await;
                    return;
                };
                let message_id = envelope.message_id.clone();
                let handler = self.handler.clone();
                let outbound = outbound.clone();
                tokio::spawn(async move {
                    let result = handler.handle(&method, envelope.payload).await;
                    let response = match result {
                        Ok(data) => {
                            ConnectionEnvelope::response(message_id.unwrap_or_default(), data)
                        }
                        Err(err) => ConnectionEnvelope::error(message_id, err),
                    };
                    let _ = outbound.send(response).await;
                });
            }
            MessageType::Event => {
                if envelope.event.as_deref() == Some(methods::DISCONNECT) {
                    tracing::info!("agent admin requested disconnect");
                }
            }
            MessageType::Response | MessageType::Error => {
                // Heartbeat acknowledgements and command responses handled here.
                if envelope.message_type == MessageType::Error {
                    if let Ok(err) = serde_json::from_value::<ApiError>(envelope.payload) {
                        tracing::warn!(code = %err.code, message = %err.message, "agent admin error");
                    }
                }
            }
        }
    }
}

async fn read_envelope(
    stream: &mut (impl futures::Stream<
        Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>,
    > + Unpin),
    deadline: Duration,
) -> Result<ConnectionEnvelope> {
    let message = tokio::time::timeout(deadline, stream.next())
        .await
        .map_err(|_| GatewayError::Timeout("handshake timed out".into()))?
        .ok_or_else(|| GatewayError::Protocol("socket closed during handshake".into()))??;
    match message {
        Message::Text(text) => ConnectionEnvelope::from_json(text.as_str()).map_err(Into::into),
        other => Err(GatewayError::Protocol(format!(
            "expected text frame during handshake, got {other:?}"
        ))),
    }
}

fn backoff(config: &ClientConfig, attempt: u32) -> Duration {
    let base = config.reconnect_initial.as_millis() as u64;
    let max = config.reconnect_max.as_millis() as u64;
    let exp = base.saturating_mul(1u64 << attempt.min(10));
    let capped = exp.min(max);
    let jitter = rand::thread_rng().gen_range(0..=capped / 4 + 1);
    Duration::from_millis((capped + jitter).min(max))
}

fn hostname() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

fn encode_state(state: ConnectionState) -> u8 {
    match state {
        ConnectionState::Disconnected => 0,
        ConnectionState::Connecting => 1,
        ConnectionState::Authenticating => 2,
        ConnectionState::Connected => 3,
        ConnectionState::Reconnecting => 4,
        ConnectionState::Failed => 5,
    }
}

fn decode_state(value: u8) -> ConnectionState {
    match value {
        1 => ConnectionState::Connecting,
        2 => ConnectionState::Authenticating,
        3 => ConnectionState::Connected,
        4 => ConnectionState::Reconnecting,
        5 => ConnectionState::Failed,
        _ => ConnectionState::Disconnected,
    }
}

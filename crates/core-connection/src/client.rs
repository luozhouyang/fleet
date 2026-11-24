use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fleet_core_auth::{save_machine_token, MachineIdentity};
use fleet_core_common::{error_codes, now, ApiError};
use fleet_core_protocol::connection::{
    capabilities, key_rotation_statement, AuthChallenge, AuthResponse, ConnectionAccepted,
    ConnectionEnvelope, ConnectionState, HelloPayload, KeyRotationProof, MessageType,
    RotateTokenPayload,
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

/// Upper bound on a machine token accepted from the admin. Generated tokens
/// are much shorter; this only bounds what is validated and persisted.
const MAX_MACHINE_TOKEN_LEN: usize = 512;

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
    pub pairing_code: Option<String>,
    pub token_file: Option<std::path::PathBuf>,
    /// Feature names advertised in `connection.hello`. Defaults to everything
    /// this daemon implements; tests and constrained builds may narrow it.
    pub capabilities: Vec<String>,
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
            pairing_code: None,
            token_file: None,
            capabilities: capabilities::SUPPORTED_BY_DAEMON
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
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
    token: RwLock<String>,
    pairing_code: RwLock<Option<String>>,
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
        let token = config.token.clone();
        let pairing_code = config.pairing_code.clone();
        Arc::new(Self {
            config,
            identity,
            rotation_identity: None,
            handler,
            state: AtomicU8::new(encode_state(ConnectionState::Disconnected)),
            state_detail: ClientState {
                connection_id: RwLock::new(None),
            },
            token: RwLock::new(token),
            pairing_code: RwLock::new(pairing_code),
            heartbeat_sequence: AtomicU64::new(0),
            event_sequence: AtomicU64::new(0),
            event_tx: broadcast::channel(1024).0,
        })
    }

    /// Attach the previous identity so the next handshake can prove a key
    /// rotation signed by the old key. `self.identity` must be the new key.
    pub fn with_rotation_identity(self: Arc<Self>, previous: MachineIdentity) -> Arc<Self> {
        // Keep the live token/code when the client has already run; fall back
        // to the configured values when the locks are busy.
        let token = self
            .token
            .try_read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| self.config.token.clone());
        let pairing_code = self
            .pairing_code
            .try_read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| self.config.pairing_code.clone());
        Arc::new(Self {
            config: self.config.clone(),
            identity: self.identity.clone(),
            rotation_identity: Some(previous),
            handler: self.handler.clone(),
            state: AtomicU8::new(self.state.load(Ordering::Relaxed)),
            state_detail: ClientState {
                connection_id: RwLock::new(None),
            },
            token: RwLock::new(token),
            pairing_code: RwLock::new(pairing_code),
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
                Err(
                    err @ (GatewayError::Revoked(_)
                    | GatewayError::NotPaired(_)
                    | GatewayError::TokenRequired(_)
                    | GatewayError::PairingCodeInvalid(_)
                    | GatewayError::TokenRejected(_)
                    | GatewayError::MachineKeyMismatch(_)
                    | GatewayError::MachineOrgMismatch(_)
                    | GatewayError::TokenSave(_)),
                ) => {
                    tracing::error!(error = %err, "machine authentication is terminal; stopping");
                    tracing::error!("{}", remediation(&err));
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
        let token = self.token.read().await.clone();
        if !token.is_empty() {
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {token}")
                    .parse()
                    .map_err(|_| GatewayError::Unauthorized("invalid token header".into()))?,
            );
        }

        let (socket, _response) = tokio::time::timeout(
            self.config.handshake_timeout,
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| GatewayError::Timeout("websocket connect timed out".into()))
        .and_then(|result| {
            result.map_err(|error| match &error {
                tokio_tungstenite::tungstenite::Error::Http(response)
                    if response.status() == 401 =>
                {
                    GatewayError::TokenRejected("machine token rejected".into())
                }
                _ => GatewayError::from(error),
            })
        })?;

        let (mut sink, mut stream) = socket.split();
        self.set_state(ConnectionState::Authenticating);

        // hello. Never send a stale pairing code alongside a valid token: the
        // gateway prioritizes pairing mode, so a used code would terminate a
        // perfectly valid reconnect.
        let pairing_code = if token.is_empty() {
            self.pairing_code
                .read()
                .await
                .clone()
                .filter(|code| !code.trim().is_empty())
        } else {
            None
        };
        let hello = HelloPayload {
            machine_id: self.config.machine_id.clone(),
            name: self.config.machine_name.clone(),
            hostname: local_hostname(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            daemon_version: self.config.daemon_version.clone(),
            protocol_version: self.config.protocol_version,
            public_key: self.identity.public_key_base64(),
            pairing_code,
            capabilities: self.config.capabilities.clone(),
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
            return Err(api_error_to_gateway(err));
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
            return Err(api_error_to_gateway(err));
        }
        if accepted_env.method.as_deref() != Some(methods::ACCEPTED) {
            return Err(GatewayError::Protocol(
                "expected connection.accepted".into(),
            ));
        }
        let accepted: ConnectionAccepted =
            serde_json::from_value(accepted_env.payload).map_err(GatewayError::from)?;
        if let Some(token) = accepted.machine_token.as_deref() {
            if let Some(path) = self.config.token_file.as_deref() {
                // A token that cannot be persisted is lost on restart. Fail
                // terminally instead of reconnecting with the old state.
                save_machine_token(path, token).map_err(|err| {
                    GatewayError::TokenSave(format!(
                        "failed to save the machine token to {}: {err}",
                        path.display()
                    ))
                })?;
            }
            *self.token.write().await = token.to_string();
            *self.pairing_code.write().await = None;
        }
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
        let event_sequence_task = Arc::clone(&event_sequence);
        let forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = forward_shutdown.cancelled() => break,
                    event = event_rx.recv() => {
                        match event {
                            Ok((name, payload, cursor)) => {
                                let sequence = event_sequence_task.fetch_add(1, Ordering::Relaxed) + 1;
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
        // Keep the per-session event sequence monotonic across reconnects so a
        // replayed outbox entry cannot be ordered behind a newer live event.
        self.event_sequence
            .store(event_sequence.load(Ordering::Relaxed), Ordering::Relaxed);
        self.set_state(ConnectionState::Disconnected);
        Ok(())
    }

    /// Persist a server-directed token rotation.
    ///
    /// The token is written to disk before it replaces the in-memory one, so a
    /// restart cannot lose a credential the admin already considers live. A
    /// daemon without a token file refuses instead of accepting a token that
    /// would vanish on restart.
    async fn apply_token_rotation(
        &self,
        payload: &serde_json::Value,
    ) -> std::result::Result<(), ApiError> {
        let rotation: RotateTokenPayload = serde_json::from_value(payload.clone())
            .map_err(|err| ApiError::invalid(format!("invalid token rotation payload: {err}")))?;
        let token = rotation.machine_token.trim().to_string();
        if token.is_empty() || token.len() > MAX_MACHINE_TOKEN_LEN || !token.is_ascii() {
            return Err(ApiError::invalid("invalid machine token"));
        }
        let Some(path) = self.config.token_file.as_deref() else {
            return Err(ApiError::new(
                error_codes::UNSUPPORTED,
                "this daemon has no machine token file to rotate",
            ));
        };
        save_machine_token(path, &token).map_err(|err| {
            ApiError::new(
                error_codes::INTERNAL,
                format!("failed to save the rotated machine token: {err}"),
            )
        })?;
        *self.token.write().await = token;
        *self.pairing_code.write().await = None;
        tracing::info!("machine token rotated");
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
                if method == methods::ROTATE_TOKEN {
                    // Handle rotations inline: the reply is the admin's proof
                    // that the new token reached the disk, so it must not race
                    // a spawned handler or an earlier rotation.
                    let response = match self.apply_token_rotation(&envelope.payload).await {
                        Ok(()) => ConnectionEnvelope::response(
                            message_id.unwrap_or_default(),
                            serde_json::json!({"ok": true}),
                        ),
                        Err(err) => ConnectionEnvelope::error(message_id, err),
                    };
                    let _ = outbound.send(response).await;
                    return;
                }
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

/// Map a server error body to the matching terminal gateway error. An
/// authorization rejection for a revoked machine is reported as `Revoked`.
fn api_error_to_gateway(error: ApiError) -> GatewayError {
    if error.code == error_codes::FORBIDDEN {
        GatewayError::Revoked(error.message)
    } else {
        GatewayError::from(error)
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

/// Actionable next step for a terminal authentication failure.
fn remediation(error: &GatewayError) -> &'static str {
    match error {
        GatewayError::TokenSave(_) => {
            "could not persist the per-machine token; fix the key directory permissions, then run `fleet daemon setup` or `fleet-daemon pair --server ... --code ...` again"
        }
        GatewayError::MachineKeyMismatch(_) => {
            "the machine key does not match the pinned key; restore the previous key and run `fleet-daemon rotate-key`, or ask an administrator to re-enroll this machine"
        }
        GatewayError::MachineOrgMismatch(_) => {
            "this machine belongs to another organization; ask an administrator to resolve the organization binding, then re-pair"
        }
        GatewayError::Revoked(_) => {
            "this machine has been revoked; an administrator must unrevoke it, then run `fleet daemon setup` or `fleet-daemon pair --server ... --code ...`"
        }
        _ => "run `fleet daemon setup` or `fleet-daemon pair --server ... --code ...` to pair this machine",
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

/// Best-effort local hostname, shared by the daemon client, `fleet-daemon
/// pair`, and `fleet daemon setup`.
pub fn local_hostname() -> String {
    #[cfg(unix)]
    if let Some(name) = system_hostname() {
        return name;
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
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

#[cfg(unix)]
fn system_hostname() -> Option<String> {
    let mut buffer = [0u8; 256];
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    let name = std::str::from_utf8(&buffer[..end]).ok()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Human-readable default for a machine's display name.
///
/// Falls back to a stable generic label instead of the machine id: ids are
/// opaque credentials-adjacent identifiers and are not shown in the UI.
pub fn default_machine_name() -> String {
    let hostname = local_hostname();
    if hostname == "unknown" || hostname.trim().is_empty() {
        "Fleet machine".to_string()
    } else {
        hostname
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingResult {
    pub machine_id: String,
    pub org_id: Option<String>,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct PairingConfig {
    pub server_url: String,
    pub machine_id: String,
    pub machine_name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub pairing_code: String,
    pub handshake_timeout: Duration,
}

/// One-shot pairing handshake. Does not start heartbeat or reconnect tasks.
pub async fn pair_machine(
    config: PairingConfig,
    identity: &MachineIdentity,
    previous: Option<&MachineIdentity>,
) -> Result<PairingResult> {
    let mut request = config
        .server_url
        .as_str()
        .into_client_request()
        .map_err(|err| GatewayError::Http(err.to_string()))?;
    request.headers_mut().remove("Authorization");

    let (socket, _) = tokio::time::timeout(
        config.handshake_timeout,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| GatewayError::Timeout("pairing connect timed out".into()))??;
    let (mut sink, mut stream) = socket.split();

    let public_key = identity.public_key_base64();
    let hello = HelloPayload {
        machine_id: config.machine_id.clone(),
        name: config.machine_name.clone(),
        hostname: config.hostname.clone(),
        os: config.os.clone(),
        arch: config.arch.clone(),
        daemon_version: config.daemon_version.clone(),
        protocol_version: config.protocol_version,
        public_key: public_key.clone(),
        pairing_code: Some(config.pairing_code.clone()),
        capabilities: capabilities::SUPPORTED_BY_DAEMON
            .iter()
            .map(|capability| (*capability).to_string())
            .collect(),
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

    let challenge_env = read_envelope(&mut stream, config.handshake_timeout).await?;
    if challenge_env.message_type == MessageType::Error {
        let error = serde_json::from_value::<ApiError>(challenge_env.payload)
            .unwrap_or_else(|_| ApiError::new(error_codes::PAIRING_CODE_INVALID, "pairing failed"));
        return Err(error.into());
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

    let rotation = previous.map(|previous| {
        let statement = key_rotation_statement(&config.machine_id, &public_key, &challenge.nonce);
        KeyRotationProof {
            new_public_key: public_key.clone(),
            old_signature: previous.sign(statement.as_bytes()),
        }
    });

    let auth = AuthResponse {
        machine_id: config.machine_id.clone(),
        challenge_id: challenge.challenge_id,
        signature: identity.sign(&nonce),
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

    let accepted_env = read_envelope(&mut stream, config.handshake_timeout).await?;
    if accepted_env.message_type == MessageType::Error {
        let error = serde_json::from_value::<ApiError>(accepted_env.payload)
            .unwrap_or_else(|_| ApiError::new(error_codes::PAIRING_CODE_INVALID, "pairing failed"));
        return Err(error.into());
    }
    if accepted_env.method.as_deref() != Some(methods::ACCEPTED) {
        return Err(GatewayError::Protocol(
            "expected connection.accepted".into(),
        ));
    }
    let accepted: ConnectionAccepted =
        serde_json::from_value(accepted_env.payload).map_err(GatewayError::from)?;
    let token = accepted
        .machine_token
        .ok_or_else(|| GatewayError::PairingCodeInvalid("server returned no token".into()))?;
    Ok(PairingResult {
        machine_id: accepted.machine_id,
        org_id: accepted.org_id,
        token,
    })
}

#[cfg(test)]
mod hostname_tests {
    use super::*;

    #[test]
    fn default_machine_name_is_never_the_unknown_placeholder() {
        let name = default_machine_name();
        assert!(!name.trim().is_empty());
        assert_ne!(name, "unknown");
    }
}

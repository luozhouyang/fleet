use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use fleet_core_auth::{
    generate_machine_token, hash_token, new_challenge, nonce_base64, verify_signature,
};
use fleet_core_common::{error_codes, now, ApiError, MachineId};
use fleet_core_protocol::connection::{
    capabilities, key_rotation_statement, AuthChallenge, AuthResponse, ConnectionAccepted,
    ConnectionEnvelope, HelloPayload, MessageType,
};
use fleet_core_protocol::machine::{HeartbeatAckPayload, HeartbeatPayload};
use fleet_core_protocol::pairing::{
    MachineAuthState, PairingCodeStatus, PairingCompletion, PairingOutcome,
};
use fleet_core_protocol::rpc::methods;
use fleet_core_protocol::PROTOCOL_VERSION;
use futures::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::error::{GatewayError, Result};
use crate::registry::{ConnectionHandle, ConnectionRegistry};

/// Verifies per-machine state, tokens and pairing codes for the gateway.
#[async_trait::async_trait]
pub trait MachineAuthenticator: Send + Sync {
    async fn machine_state(
        &self,
        machine_id: &str,
    ) -> std::result::Result<MachineAuthState, ApiError>;
    async fn pin_key(
        &self,
        machine_id: &str,
        public_key: &str,
    ) -> std::result::Result<(), ApiError>;
    async fn is_revoked(&self, machine_id: &str) -> std::result::Result<bool, ApiError>;

    async fn machine_for_token(&self, token: &str)
        -> std::result::Result<Option<String>, ApiError>;

    /// Called after the handshake proved both the token and the pinned key.
    ///
    /// A two-phase token rotation stores the new hash as pending until the
    /// daemon confirms it. When the daemon later reconnects with that token,
    /// this hook promotes it to the live credential. Promoting here, after
    /// signature verification, means a leaked pending token alone cannot
    /// invalidate the current credential.
    async fn confirm_machine_token(
        &self,
        _machine_id: &str,
        _token: &str,
    ) -> std::result::Result<(), ApiError> {
        Ok(())
    }

    async fn pairing_code(&self, code: &str) -> std::result::Result<PairingCodeStatus, ApiError>;
    async fn record_pairing_failure(&self, code_id: &str) -> std::result::Result<(), ApiError>;
    async fn complete_pairing(
        &self,
        request: PairingCompletion,
    ) -> std::result::Result<PairingOutcome, ApiError>;
}

/// Events emitted by the gateway for the agent admin to consume.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    MachineConnected {
        machine_id: MachineId,
        hello: HelloPayload,
        connection_id: String,
    },
    MachineDisconnected {
        machine_id: MachineId,
        connection_id: String,
        reason: Option<String>,
    },
    MachineEvent {
        machine_id: MachineId,
        event: String,
        sequence: u64,
        /// Outbox sequence of a durable event, when the daemon attached one.
        cursor: Option<u64>,
        payload: serde_json::Value,
    },
    Heartbeat {
        machine_id: MachineId,
        at: fleet_core_common::Timestamp,
    },
}

pub struct ConnectionGateway {
    authenticator: Arc<dyn MachineAuthenticator>,
    registry: ConnectionRegistry,
    events: broadcast::Sender<GatewayEvent>,
    heartbeat_interval: Duration,
    heartbeat_timeout: Duration,
    handshake_timeout: Duration,
    outbound_capacity: usize,
    limiter: UpgradeLimiter,
}

impl ConnectionGateway {
    pub fn new(
        authenticator: Arc<dyn MachineAuthenticator>,
        heartbeat_interval: Duration,
        heartbeat_timeout: Duration,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            authenticator,
            registry: ConnectionRegistry::with_request_timeout(Duration::from_secs(30)),
            events,
            heartbeat_interval,
            heartbeat_timeout,
            handshake_timeout: Duration::from_secs(10),
            outbound_capacity: 256,
            limiter: UpgradeLimiter::default(),
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

    async fn serve_socket(self: Arc<Self>, auth: UpgradeAuth, socket: WebSocket) {
        let (sink, mut stream) = socket.split();
        match self.handshake(auth, sink, &mut stream).await {
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
        auth: UpgradeAuth,
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
        if !fleet_core_protocol::is_supported_protocol_version(hello.protocol_version) {
            return Err(GatewayError::Protocol(format!(
                "unsupported protocol version {} (server speaks {}, supports {}..={})",
                hello.protocol_version,
                PROTOCOL_VERSION,
                fleet_core_protocol::MIN_SUPPORTED_PROTOCOL_VERSION,
                PROTOCOL_VERSION
            )));
        }
        if hello.machine_id.trim().is_empty() || hello.public_key.trim().is_empty() {
            return Err(GatewayError::Protocol(
                "machine_id and public_key are required".into(),
            ));
        }
        if hello.capabilities.len() > MAX_CAPABILITIES {
            return Err(GatewayError::Protocol(
                "too many advertised capabilities".into(),
            ));
        }
        let capabilities = negotiate_capabilities(&hello.capabilities);
        if hello
            .pairing_code
            .as_deref()
            .is_some_and(|code| code.chars().count() > 32)
        {
            return Err(GatewayError::Protocol("pairing_code is too long".into()));
        }
        if self.authenticator.is_revoked(&hello.machine_id).await? {
            let error = ApiError::new(error_codes::FORBIDDEN, "machine is revoked");
            let _ = sink
                .send(Message::text(
                    ConnectionEnvelope::error(hello_env.message_id.clone(), error).to_json(),
                ))
                .await;
            return Err(GatewayError::Revoked(hello.machine_id));
        }

        // 2. Resolve the authentication mode. The pairing code is validated
        // before the challenge, but it is only consumed after the signature
        // and rotation checks succeed.
        let state = self.authenticator.machine_state(&hello.machine_id).await?;
        let pinned_key = state.pinned_key;

        enum Mode {
            Pairing(fleet_core_protocol::pairing::PairingCodeInfo),
            Token,
        }

        // A present-but-empty/whitespace code is treated as absent: an empty
        // `FLEET_PAIR_CODE` must not force pairing mode.
        let pairing_code = hello
            .pairing_code
            .as_deref()
            .map(str::trim)
            .filter(|code| !code.is_empty());
        let mode = if let Some(code) = pairing_code {
            if let UpgradeAuth::MachineToken { machine_id, .. } = &auth {
                if machine_id != &hello.machine_id {
                    send_error_frame(
                        &mut sink,
                        hello_env.message_id.clone(),
                        error_codes::MACHINE_KEY_MISMATCH,
                        "machine token does not match the hello machine id",
                    )
                    .await;
                    return Err(GatewayError::MachineKeyMismatch(
                        "machine token/id mismatch".into(),
                    ));
                }
            }
            match self.authenticator.pairing_code(code).await? {
                PairingCodeStatus::Valid(info) => Mode::Pairing(info),
                _ => {
                    let error = ApiError::new(
                        error_codes::PAIRING_CODE_INVALID,
                        "pairing code is not valid",
                    );
                    let _ = sink
                        .send(Message::text(
                            ConnectionEnvelope::error(hello_env.message_id.clone(), error)
                                .to_json(),
                        ))
                        .await;
                    return Err(GatewayError::PairingCodeInvalid("invalid code".into()));
                }
            }
        } else {
            match &auth {
                UpgradeAuth::MachineToken { machine_id, .. } => {
                    if machine_id != &hello.machine_id {
                        send_error_frame(
                            &mut sink,
                            hello_env.message_id.clone(),
                            error_codes::MACHINE_KEY_MISMATCH,
                            "machine token does not match the hello machine id",
                        )
                        .await;
                        return Err(GatewayError::MachineKeyMismatch(
                            "machine token/id mismatch".into(),
                        ));
                    }
                    if pinned_key.is_none() {
                        let error = ApiError::new(
                            error_codes::MACHINE_NOT_PAIRED,
                            "machine has no pinned key",
                        );
                        let _ = sink
                            .send(Message::text(
                                ConnectionEnvelope::error(hello_env.message_id.clone(), error)
                                    .to_json(),
                            ))
                            .await;
                        return Err(GatewayError::NotPaired(hello.machine_id));
                    }
                    Mode::Token
                }
                UpgradeAuth::Pairing => {
                    let code = if pinned_key.is_some() {
                        error_codes::MACHINE_TOKEN_REQUIRED
                    } else {
                        error_codes::MACHINE_NOT_PAIRED
                    };
                    let error = ApiError::new(code, "machine token or pairing code required");
                    let _ = sink
                        .send(Message::text(
                            ConnectionEnvelope::error(hello_env.message_id.clone(), error)
                                .to_json(),
                        ))
                        .await;
                    return Err(if pinned_key.is_some() {
                        GatewayError::TokenRequired(hello.machine_id)
                    } else {
                        GatewayError::NotPaired(hello.machine_id)
                    });
                }
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
        let auth_response: AuthResponse = serde_json::from_value(auth_env.payload)?;
        if auth_response.machine_id != hello.machine_id {
            send_error_frame(
                &mut sink,
                hello_env.message_id.clone(),
                error_codes::MACHINE_KEY_MISMATCH,
                "machine id in the auth response does not match the hello",
            )
            .await;
            return Err(GatewayError::MachineKeyMismatch(
                "machine id mismatch".into(),
            ));
        }
        if auth_response.challenge_id != challenge.id {
            // A mismatched challenge id cannot succeed on retry. Report it as a
            // terminal error (pairing-code invalid while pairing, key mismatch
            // otherwise) instead of a frameless retryable `Auth`.
            if let Mode::Pairing(_) = &mode {
                send_error_frame(
                    &mut sink,
                    hello_env.message_id.clone(),
                    error_codes::PAIRING_CODE_INVALID,
                    "the auth response references an unknown challenge id",
                )
                .await;
                return Err(GatewayError::PairingCodeInvalid(
                    "unknown challenge id".into(),
                ));
            }
            send_error_frame(
                &mut sink,
                hello_env.message_id.clone(),
                error_codes::MACHINE_KEY_MISMATCH,
                "the auth response references an unknown challenge id; re-pair this machine",
            )
            .await;
            return Err(GatewayError::MachineKeyMismatch(
                "unknown challenge id".into(),
            ));
        }
        if verify_signature(
            &hello.public_key,
            &challenge.nonce,
            &auth_response.signature,
        )
        .is_err()
        {
            if let Mode::Pairing(info) = &mode {
                let _ = self.authenticator.record_pairing_failure(&info.id).await;
                // Report the failure to the client as a terminal pairing error
                // instead of dropping the socket without a frame. The code is
                // not consumed here.
                send_error_frame(
                    &mut sink,
                    hello_env.message_id.clone(),
                    error_codes::PAIRING_CODE_INVALID,
                    "pairing signature was rejected",
                )
                .await;
                return Err(GatewayError::PairingCodeInvalid(
                    "invalid challenge signature".into(),
                ));
            }
            return Err(GatewayError::Auth("invalid challenge signature".into()));
        }

        // 5. Rotation requires a proof signed by the previously pinned key.
        let rotation_required = matches!(
            &pinned_key,
            Some(pinned) if pinned != &hello.public_key
        );
        if rotation_required {
            let Some(proof) = auth_response.rotation.as_ref() else {
                send_error_frame(
                    &mut sink,
                    hello_env.message_id.clone(),
                    error_codes::MACHINE_KEY_MISMATCH,
                    "the presented key differs from the pinned key and no rotation proof was supplied",
                )
                .await;
                return Err(GatewayError::MachineKeyMismatch(
                    "key rotation proof required".into(),
                ));
            };
            if proof.new_public_key != hello.public_key {
                send_error_frame(
                    &mut sink,
                    hello_env.message_id.clone(),
                    error_codes::MACHINE_KEY_MISMATCH,
                    "the rotation proof does not cover the presented key",
                )
                .await;
                return Err(GatewayError::MachineKeyMismatch(
                    "rotation proof key mismatch".into(),
                ));
            }
            let statement = key_rotation_statement(
                &hello.machine_id,
                &hello.public_key,
                &challenge_payload.nonce,
            );
            if let Err(err) = verify_signature(
                pinned_key.as_deref().unwrap_or_default(),
                statement.as_bytes(),
                &proof.old_signature,
            ) {
                send_error_frame(
                    &mut sink,
                    hello_env.message_id.clone(),
                    error_codes::MACHINE_KEY_MISMATCH,
                    "the rotation proof was not signed by the pinned key",
                )
                .await;
                return Err(GatewayError::MachineKeyMismatch(format!(
                    "invalid rotation proof: {err}"
                )));
            }
        }

        // 6. Consume the pairing code or persist a verified rotation.
        let mut issued_token = None;
        let mut issued_org = None;
        match mode {
            Mode::Pairing(info) => {
                let token = generate_machine_token();
                let outcome = match self
                    .authenticator
                    .complete_pairing(PairingCompletion {
                        code_id: info.id,
                        machine_id: hello.machine_id.clone(),
                        public_key: hello.public_key.clone(),
                        token_hash: hash_token(&token),
                        machine_name: hello.name.clone(),
                        hostname: hello.hostname.clone(),
                        os: hello.os.clone(),
                        arch: hello.arch.clone(),
                        daemon_version: hello.daemon_version.clone(),
                        protocol_version: hello.protocol_version,
                        rotation_verified: rotation_required,
                    })
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        let _ = sink
                            .send(Message::text(
                                ConnectionEnvelope::error(
                                    hello_env.message_id.clone(),
                                    error.clone(),
                                )
                                .to_json(),
                            ))
                            .await;
                        return Err(GatewayError::from(error));
                    }
                };
                issued_token = Some(token);
                issued_org = Some(outcome.org_id);
            }
            Mode::Token => {
                if rotation_required {
                    if let Err(error) = self
                        .authenticator
                        .pin_key(&hello.machine_id, &hello.public_key)
                        .await
                    {
                        let _ = sink
                            .send(Message::text(
                                ConnectionEnvelope::error(
                                    hello_env.message_id.clone(),
                                    error.clone(),
                                )
                                .to_json(),
                            ))
                            .await;
                        return Err(GatewayError::from(error));
                    }
                }
                // The signature check above proved key ownership. Only now is
                // a pending rotation token allowed to become the live token.
                if let UpgradeAuth::MachineToken { token, .. } = &auth {
                    if let Err(error) = self
                        .authenticator
                        .confirm_machine_token(&hello.machine_id, token)
                        .await
                    {
                        let _ = sink
                            .send(Message::text(
                                ConnectionEnvelope::error(
                                    hello_env.message_id.clone(),
                                    error.clone(),
                                )
                                .to_json(),
                            ))
                            .await;
                        return Err(GatewayError::from(error));
                    }
                }
            }
        }

        // 7. register the live connection
        let (outbound_tx, outbound_rx) = mpsc::channel(self.outbound_capacity);
        let connection_id = fleet_core_common::new_id("conn");
        let handle = Arc::new(ConnectionHandle {
            machine_id: hello.machine_id.clone(),
            connection_id: connection_id.clone(),
            connected_at: now(),
            sender: outbound_tx,
            pending: dashmap::DashMap::new(),
            last_seen_millis: std::sync::atomic::AtomicI64::new(now().timestamp_millis()),
            shutdown: CancellationToken::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
            capabilities: capabilities.clone(),
        });
        // 8. accepted
        sink.send(Message::text(
            ConnectionEnvelope::request(
                methods::ACCEPTED,
                serde_json::to_value(ConnectionAccepted {
                    connection_id: connection_id.clone(),
                    machine_id: hello.machine_id.clone(),
                    server_version: env!("CARGO_PKG_VERSION").to_string(),
                    protocol_version: PROTOCOL_VERSION,
                    heartbeat_interval_ms: self.heartbeat_interval.as_millis() as u64,
                    machine_token: issued_token,
                    org_id: issued_org,
                    capabilities: capabilities.clone(),
                })?,
            )
            .to_json(),
        ))
        .await
        .map_err(|e| GatewayError::WebSocket(e.to_string()))?;

        // 9. register the connection and start the writer
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
            connection_id: handle.connection_id.clone(),
            reason: reason.clone(),
        });
        let _ = self.events.send(GatewayEvent::MachineEvent {
            machine_id: machine_id.clone(),
            event: "connection.closed".into(),
            sequence: 0,
            cursor: None,
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
                        sequence: envelope.sequence.unwrap_or(0),
                        cursor: envelope.cursor,
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
                    if let Err(err) = handle.sender.try_send(response) {
                        tracing::debug!(error = %err, machine = %handle.machine_id, "heartbeat ack dropped");
                    }
                }
                other => {
                    let response = ConnectionEnvelope::error(
                        envelope.message_id,
                        ApiError::new(
                            error_codes::UNSUPPORTED,
                            format!(
                                "method `{}` is not supported by the agent admin",
                                other.unwrap_or("<missing>")
                            ),
                        ),
                    );
                    if let Err(err) = handle.sender.try_send(response) {
                        tracing::debug!(error = %err, machine = %handle.machine_id, "error response dropped");
                    }
                }
            },
        }
    }
}

/// Best-effort error envelope so the client sees an actionable code instead of
/// a bare socket close.
async fn send_error_frame<S>(sink: &mut S, message_id: Option<String>, code: &str, message: &str)
where
    S: futures::Sink<Message, Error = axum::Error> + Unpin,
{
    let error = ApiError::new(code, message);
    let _ = sink
        .send(Message::text(
            ConnectionEnvelope::error(message_id, error).to_json(),
        ))
        .await;
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

#[derive(Clone)]
enum UpgradeAuth {
    /// Authenticated upgrade; the raw credential is kept so a confirmed
    /// rotation can be promoted after the challenge signature is verified.
    MachineToken {
        machine_id: String,
        token: String,
    },
    Pairing,
}

struct PeerAddr(std::net::IpAddr);

impl<S> axum::extract::FromRequestParts<S> for PeerAddr
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|info| info.0.ip())
                .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        ))
    }
}

/// Upper bound on a daemon's advertised capability list. The list is small on
/// every supported daemon; this only bounds parsing work.
const MAX_CAPABILITIES: usize = 32;

/// Keep capabilities both sides implement, in the admin's stable order.
fn negotiate_capabilities(offered: &[String]) -> Vec<String> {
    capabilities::SUPPORTED_BY_ADMIN
        .iter()
        .filter(|capability| offered.iter().any(|value| value == *capability))
        .map(|capability| (*capability).to_string())
        .collect()
}

fn bearer_credentials(header: &str) -> Option<&str> {
    let (scheme, credentials) = header.trim().split_once(' ')?;
    let credentials = credentials.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !credentials.is_empty()).then_some(credentials)
}

/// Extract bearer credentials from an `Authorization` header.
///
/// `Ok(None)` means the header is absent (pairing mode); any present but
/// malformed header is an error, including a non-UTF-8 value.
fn authorization_credentials(
    headers: &HeaderMap,
) -> std::result::Result<Option<&str>, &'static str> {
    match headers.get(axum::http::header::AUTHORIZATION) {
        None => Ok(None),
        Some(value) => {
            let header = value.to_str().map_err(|_| "invalid authorization header")?;
            bearer_credentials(header)
                .ok_or("invalid bearer header")
                .map(Some)
        }
    }
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(ApiError::new(error_codes::UNAUTHORIZED, message)),
    )
        .into_response()
}

async fn machine_upgrade(
    State(gateway): State<Arc<ConnectionGateway>>,
    PeerAddr(peer): PeerAddr,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let auth = match authorization_credentials(&headers) {
        Err(message) => return unauthorized(message),
        Ok(None) => UpgradeAuth::Pairing,
        Ok(Some(credentials)) => match gateway.authenticator.machine_for_token(credentials).await {
            Ok(Some(machine_id)) => UpgradeAuth::MachineToken {
                machine_id,
                token: credentials.to_string(),
            },
            Ok(None) => return unauthorized("invalid machine token"),
            Err(error) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(error)).into_response()
            }
        },
    };

    // Only credential-less upgrades consume the per-IP budget. Authenticated
    // machine reconnects must not be throttled by unrelated pairing attempts
    // (or by an attacker sharing the NAT address).
    if matches!(auth, UpgradeAuth::Pairing) && !gateway.limiter.allow(peer) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            axum::Json(ApiError::new(
                error_codes::INVALID_REQUEST,
                "too many machine upgrade attempts",
            )),
        )
            .into_response();
    }

    ws.on_upgrade(move |socket| async move { gateway.serve_socket(auth, socket).await })
}

const LIMITER_WINDOW: Duration = Duration::from_secs(600);
const LIMITER_LIMIT: usize = 30;
const LIMITER_SWEEP_EVERY: usize = 1024;

#[derive(Default)]
struct UpgradeLimiter {
    hits: dashmap::DashMap<std::net::IpAddr, std::collections::VecDeque<std::time::Instant>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl UpgradeLimiter {
    fn allow(&self, ip: std::net::IpAddr) -> bool {
        self.evict_stale();
        let now = std::time::Instant::now();
        let mut entry = self.hits.entry(ip).or_default();
        while entry
            .front()
            .is_some_and(|at| now.duration_since(*at) > LIMITER_WINDOW)
        {
            entry.pop_front();
        }
        if entry.len() >= LIMITER_LIMIT {
            return false;
        }
        entry.push_back(now);
        true
    }

    /// Periodically drop IP buckets whose window has fully expired. Without
    /// this the map grows once per source IP for the process lifetime.
    fn evict_stale(&self) {
        if !self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(LIMITER_SWEEP_EVERY)
        {
            return;
        }
        let now = std::time::Instant::now();
        self.hits.retain(|_, entry| {
            while entry
                .front()
                .is_some_and(|at| now.duration_since(*at) > LIMITER_WINDOW)
            {
                entry.pop_front();
            }
            !entry.is_empty()
        });
    }
}

/// Helper for fleet-admin state: implements the authenticator over closures.
///
/// Closure-based authenticators only support pinned keys; token and pairing
/// lookups are reported as absent, so such a gateway never accepts pairing.
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
    async fn machine_state(
        &self,
        machine_id: &str,
    ) -> std::result::Result<MachineAuthState, ApiError> {
        Ok(MachineAuthState {
            pinned_key: (self.pinned)(machine_id.to_string()).await?,
        })
    }

    async fn pin_key(
        &self,
        machine_id: &str,
        public_key: &str,
    ) -> std::result::Result<(), ApiError> {
        (self.pin)(machine_id.to_string(), public_key.to_string()).await
    }

    async fn is_revoked(&self, _machine_id: &str) -> std::result::Result<bool, ApiError> {
        Ok(false)
    }

    async fn machine_for_token(
        &self,
        _token: &str,
    ) -> std::result::Result<Option<String>, ApiError> {
        Ok(None)
    }

    async fn pairing_code(&self, _code: &str) -> std::result::Result<PairingCodeStatus, ApiError> {
        Ok(PairingCodeStatus::Invalid)
    }

    async fn record_pairing_failure(&self, _code_id: &str) -> std::result::Result<(), ApiError> {
        Ok(())
    }

    async fn complete_pairing(
        &self,
        _request: PairingCompletion,
    ) -> std::result::Result<PairingOutcome, ApiError> {
        Err(ApiError::new(
            error_codes::INTERNAL,
            "this authenticator does not support pairing",
        ))
    }
}

#[cfg(test)]
mod limiter_tests {
    use super::*;

    #[test]
    fn upgrade_limiter_rejects_after_thirty_attempts() {
        let limiter = UpgradeLimiter::default();
        let ip = std::net::IpAddr::from([127, 0, 0, 1]);
        for _ in 0..30 {
            assert!(limiter.allow(ip));
        }
        assert!(!limiter.allow(ip));
    }

    #[test]
    fn missing_authorization_selects_pairing() {
        let headers = HeaderMap::new();
        assert_eq!(authorization_credentials(&headers), Ok(None));
    }

    #[test]
    fn malformed_and_non_utf8_authorization_are_rejected() {
        let mut headers = HeaderMap::new();
        for value in ["Basic abc", "Bearer", "Bearer   "] {
            headers.insert(
                axum::http::header::AUTHORIZATION,
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
            assert!(authorization_credentials(&headers).is_err(), "{value:?}");
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
        );
        assert_eq!(
            authorization_credentials(&headers),
            Err("invalid authorization header"),
            "a non-UTF-8 header must be rejected, not treated as absent"
        );
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use fleet_core_auth::MachineIdentity;
use fleet_core_common::{error_codes, ApiError};
use fleet_core_connection::{
    ClientConfig, ConnectionGateway, ConnectionState, DaemonRequestHandler, GatewayError,
    MachineAuthenticator, MachineConnectionClient,
};
use fleet_core_protocol::connection::{
    AuthResponse, ConnectionAccepted, ConnectionEnvelope, HelloPayload, MessageType,
};
use fleet_core_protocol::pairing::{
    MachineAuthState, PairingCodeInfo, PairingCodeStatus, PairingCompletion, PairingOutcome,
};
use fleet_core_protocol::rpc::methods;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MemoryAuth {
    keys: Mutex<HashMap<String, String>>,
    tokens: Mutex<HashMap<String, String>>, // token hash -> machine id
    codes: Mutex<HashMap<String, PairingCodeInfo>>, // code hash -> info
    code_ids: Mutex<HashMap<String, String>>, // code id -> code hash
    failures: Mutex<HashMap<String, u32>>,  // code id -> failed signatures
    consumed: Mutex<std::collections::HashSet<String>>, // consumed code hashes
    revoked: Mutex<std::collections::HashSet<String>>,
    complete_pairing_error: Mutex<Option<ApiError>>,
    pin_key_error: Mutex<Option<ApiError>>,
}

impl MemoryAuth {
    fn register(&self, machine_id: &str, public_key: &str, token: &str) {
        self.keys
            .lock()
            .unwrap()
            .insert(machine_id.into(), public_key.into());
        self.tokens
            .lock()
            .unwrap()
            .insert(fleet_core_auth::hash_token(token), machine_id.into());
    }
}

#[async_trait]
impl MachineAuthenticator for MemoryAuth {
    async fn machine_state(&self, machine_id: &str) -> Result<MachineAuthState, ApiError> {
        Ok(MachineAuthState {
            pinned_key: self.keys.lock().unwrap().get(machine_id).cloned(),
        })
    }

    async fn pin_key(&self, machine_id: &str, public_key: &str) -> Result<(), ApiError> {
        if let Some(error) = self.pin_key_error.lock().unwrap().clone() {
            return Err(error);
        }
        self.keys
            .lock()
            .unwrap()
            .insert(machine_id.into(), public_key.into());
        Ok(())
    }

    async fn is_revoked(&self, machine_id: &str) -> Result<bool, ApiError> {
        Ok(self.revoked.lock().unwrap().contains(machine_id))
    }

    async fn machine_for_token(&self, token: &str) -> Result<Option<String>, ApiError> {
        Ok(self
            .tokens
            .lock()
            .unwrap()
            .get(&fleet_core_auth::hash_token(token))
            .cloned())
    }

    async fn pairing_code(&self, code: &str) -> Result<PairingCodeStatus, ApiError> {
        let Some(hash) = fleet_core_auth::hash_pair_code(code) else {
            return Ok(PairingCodeStatus::Invalid);
        };
        let Some(info) = self.codes.lock().unwrap().get(&hash).cloned() else {
            return Ok(PairingCodeStatus::Invalid);
        };
        if self.consumed.lock().unwrap().contains(&hash) {
            return Ok(PairingCodeStatus::Used {
                id: info.id,
                org_id: info.org_id,
            });
        }
        if info.expires_at <= fleet_core_common::now() {
            return Ok(PairingCodeStatus::Expired {
                id: info.id,
                org_id: info.org_id,
            });
        }
        let failures = self
            .failures
            .lock()
            .unwrap()
            .get(&info.id)
            .copied()
            .unwrap_or(0);
        if failures >= 5 {
            return Ok(PairingCodeStatus::Locked {
                id: info.id,
                org_id: info.org_id,
            });
        }
        Ok(PairingCodeStatus::Valid(PairingCodeInfo {
            attempts_left: 5 - failures,
            ..info
        }))
    }

    async fn record_pairing_failure(&self, code_id: &str) -> Result<(), ApiError> {
        *self
            .failures
            .lock()
            .unwrap()
            .entry(code_id.to_string())
            .or_insert(0) += 1;
        Ok(())
    }

    async fn complete_pairing(
        &self,
        request: PairingCompletion,
    ) -> Result<PairingOutcome, ApiError> {
        if let Some(error) = self.complete_pairing_error.lock().unwrap().clone() {
            return Err(error);
        }
        if let Some(hash) = self.code_ids.lock().unwrap().get(&request.code_id) {
            self.consumed.lock().unwrap().insert(hash.clone());
        }
        self.keys
            .lock()
            .unwrap()
            .insert(request.machine_id.clone(), request.public_key);
        let mut tokens = self.tokens.lock().unwrap();
        // Re-pairing replaces the machine's token hash, so the previous token
        // no longer resolves.
        tokens.retain(|_, machine_id| machine_id != &request.machine_id);
        tokens.insert(request.token_hash, request.machine_id.clone());
        drop(tokens);
        Ok(PairingOutcome {
            machine_id: request.machine_id,
            org_id: "org_default".into(),
            paired_at: fleet_core_common::now(),
        })
    }
}

struct EchoHandler;

#[async_trait]
impl DaemonRequestHandler for EchoHandler {
    async fn handle(&self, method: &str, payload: Value) -> Result<Value, ApiError> {
        if method == "fail" {
            return Err(ApiError::new("boom", "intentional failure"));
        }
        Ok(json!({"method": method, "payload": payload}))
    }

    async fn on_connected(&self, _accepted: &ConnectionAccepted) {}
}

async fn start_gateway() -> (Arc<ConnectionGateway>, Arc<MemoryAuth>, String) {
    let auth = Arc::new(MemoryAuth::default());
    let (gateway, url, _handle) = start_gateway_with(auth.clone()).await;
    (gateway, auth, url)
}

async fn start_gateway_with(
    auth: Arc<dyn MachineAuthenticator>,
) -> (Arc<ConnectionGateway>, String, tokio::task::JoinHandle<()>) {
    let gateway = ConnectionGateway::new(auth, Duration::from_millis(500), Duration::from_secs(5));
    let app = gateway.clone().router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (gateway, format!("ws://{addr}/machine"), handle)
}

async fn start_gateway_on(
    auth: Arc<dyn MachineAuthenticator>,
    address: std::net::SocketAddr,
) -> (Arc<ConnectionGateway>, tokio::task::JoinHandle<()>) {
    let gateway = ConnectionGateway::new(auth, Duration::from_millis(500), Duration::from_secs(5));
    let app = gateway.clone().router();
    let mut last_error = None;
    for _ in 0..50 {
        match tokio::net::TcpListener::bind(address).await {
            Ok(listener) => {
                let handle = tokio::spawn(async move {
                    let _ = axum::serve(listener, app).await;
                });
                return (gateway, handle);
            }
            Err(err) => {
                last_error = Some(err);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    panic!("could not rebind test gateway: {last_error:?}");
}

fn client_config(url: String, token: &str) -> ClientConfig {
    let mut config = ClientConfig::new(url, token, "m1", "test-machine");
    config.heartbeat_interval = Duration::from_millis(300);
    config.reconnect_initial = Duration::from_millis(100);
    config.reconnect_max = Duration::from_millis(500);
    config.handshake_timeout = Duration::from_secs(5);
    config
}

async fn wait_connected(client: &MachineConnectionClient) {
    for _ in 0..100 {
        if client.state() == ConnectionState::Connected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("client did not connect, state = {:?}", client.state());
}

async fn wait_finished(task: &tokio::task::JoinHandle<()>, label: &str) {
    for _ in 0..100 {
        if task.is_finished() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{label} did not stop");
}

fn test_hello(machine_id: &str, public_key: &str, pairing_code: Option<String>) -> HelloPayload {
    HelloPayload {
        machine_id: machine_id.to_string(),
        name: "test".into(),
        hostname: "host".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        daemon_version: "test".into(),
        protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
        public_key: public_key.to_string(),
        pairing_code,
    }
}

fn pairing_config(
    url: String,
    machine_id: &str,
    code: &str,
) -> fleet_core_connection::PairingConfig {
    fleet_core_connection::PairingConfig {
        server_url: url,
        machine_id: machine_id.into(),
        machine_name: "paired".into(),
        hostname: "host".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        daemon_version: "test".into(),
        protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
        pairing_code: code.into(),
        handshake_timeout: Duration::from_secs(5),
    }
}

fn insert_code(auth: &MemoryAuth, code: &str) -> String {
    let hash = fleet_core_auth::hash_pair_code(code).unwrap();
    let id = format!("pair_{}", &hash[..8]);
    auth.code_ids
        .lock()
        .unwrap()
        .insert(id.clone(), hash.clone());
    auth.codes.lock().unwrap().insert(
        hash,
        PairingCodeInfo {
            id: id.clone(),
            org_id: "org_default".into(),
            created_by: "admin".into(),
            machine_name_hint: None,
            expires_at: fleet_core_common::now() + chrono::Duration::seconds(60),
            attempts_left: 5,
        },
    );
    id
}

fn insert_expired_code(auth: &MemoryAuth, code: &str) {
    let hash = fleet_core_auth::hash_pair_code(code).unwrap();
    auth.codes.lock().unwrap().insert(
        hash,
        PairingCodeInfo {
            id: "pair_expired".into(),
            org_id: "org_default".into(),
            created_by: "admin".into(),
            machine_name_hint: None,
            expires_at: fleet_core_common::now() - chrono::Duration::seconds(1),
            attempts_left: 5,
        },
    );
}

async fn raw_hello_error(url: &str, hello: HelloPayload) -> ApiError {
    let request = url.into_client_request().expect("client request");
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");
    socket
        .send(Message::Text(
            ConnectionEnvelope::request(methods::HELLO, json!(hello))
                .to_json()
                .into(),
        ))
        .await
        .expect("hello");
    let frame = socket.next().await.expect("frame").expect("frame");
    let envelope: ConnectionEnvelope =
        serde_json::from_str(frame.to_text().expect("text")).expect("envelope");
    assert_eq!(envelope.message_type, MessageType::Error);
    serde_json::from_value(envelope.payload).expect("api error")
}

async fn raw_upgrade_error(url: &str, token: Option<&str>, hello: HelloPayload) -> ApiError {
    let mut request = url.into_client_request().expect("client request");
    if let Some(token) = token {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}").parse().expect("header"),
        );
    }
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");
    socket
        .send(Message::Text(
            ConnectionEnvelope::request(methods::HELLO, json!(hello))
                .to_json()
                .into(),
        ))
        .await
        .expect("hello");
    let frame = socket.next().await.expect("frame").expect("frame");
    let envelope: ConnectionEnvelope =
        serde_json::from_str(frame.to_text().expect("text")).expect("envelope");
    assert_eq!(envelope.message_type, MessageType::Error);
    serde_json::from_value(envelope.payload).expect("api error")
}

/// Run one raw pairing handshake and sign the wrong bytes.
async fn bad_signature_attempt(url: &str, code: &str, machine_id: &str) {
    let request = url.into_client_request().expect("client request");
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");
    let identity = MachineIdentity::generate();
    let hello = test_hello(machine_id, &identity.public_key_base64(), Some(code.into()));
    socket
        .send(Message::Text(
            ConnectionEnvelope::request(methods::HELLO, json!(hello))
                .to_json()
                .into(),
        ))
        .await
        .expect("hello");
    let challenge = socket.next().await.expect("challenge").expect("frame");
    let challenge: ConnectionEnvelope =
        serde_json::from_str(challenge.to_text().expect("text")).expect("envelope");
    let challenge: fleet_core_protocol::connection::AuthChallenge =
        serde_json::from_value(challenge.payload).expect("challenge payload");
    let response = AuthResponse {
        machine_id: machine_id.to_string(),
        challenge_id: challenge.challenge_id,
        signature: identity.sign(b"not the challenge nonce"),
        rotation: None,
    };
    socket
        .send(Message::Text(
            ConnectionEnvelope::request(methods::AUTH_RESPONSE, json!(response))
                .to_json()
                .into(),
        ))
        .await
        .expect("auth");
    let frame = socket.next().await.expect("error frame").expect("frame");
    let envelope: ConnectionEnvelope =
        serde_json::from_str(frame.to_text().expect("text")).expect("envelope");
    assert_eq!(envelope.message_type, MessageType::Error);
    let error: ApiError = serde_json::from_value(envelope.payload).expect("api error");
    assert_eq!(error.code, error_codes::PAIRING_CODE_INVALID);
}

#[tokio::test]
async fn machine_connects_and_routes_requests() {
    let (gateway, auth, url) = start_gateway().await;
    let registry = gateway.registry();
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    let client = MachineConnectionClient::new(
        client_config(url, "test-token"),
        identity,
        Arc::new(EchoHandler),
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));

    wait_connected(&client).await;
    assert!(registry.is_online("m1"));

    let response = registry
        .request("m1", "echo", json!({"x": 1}))
        .await
        .unwrap();
    assert_eq!(response["method"], "echo");
    assert_eq!(response["payload"]["x"], 1);

    let error = registry.request("m1", "fail", json!({})).await.unwrap_err();
    assert_eq!(error.code, "boom");

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    for _ in 0..100 {
        if !registry.is_online("m1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!registry.is_online("m1"));
}

#[tokio::test]
async fn durable_event_cursor_reaches_the_gateway_consumer() {
    let (gateway, auth, url) = start_gateway().await;
    let mut events = gateway.subscribe();
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    let client = MachineConnectionClient::new(
        client_config(url, "test-token"),
        identity,
        Arc::new(EchoHandler),
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_connected(&client).await;

    client.send_event_with_cursor("agent.event", json!({"ok": true}), 9);

    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(fleet_core_connection::GatewayEvent::MachineEvent {
                    event,
                    cursor,
                    payload,
                    ..
                }) if event == "agent.event" => break (cursor, payload),
                Ok(_) => continue,
                Err(error) => panic!("gateway event stream failed: {error}"),
            }
        }
    })
    .await
    .expect("event should arrive");
    assert_eq!(event.0, Some(9));
    assert_eq!(event.1["ok"], true);

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn revoked_machine_is_rejected_and_stops_reconnecting() {
    let auth = Arc::new(MemoryAuth::default());
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    auth.revoked.lock().unwrap().insert("m1".to_string());
    let (gateway, url, _server) = start_gateway_with(auth).await;
    let client = MachineConnectionClient::new(
        client_config(url, "test-token"),
        identity,
        Arc::new(EchoHandler),
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));

    for _ in 0..100 {
        if task.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        task.is_finished(),
        "revoked client must stop, state = {:?}",
        client.state()
    );
    assert!(!gateway.registry().is_online("m1"));
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn rejects_bad_token() {
    let (_gateway, _auth, url) = start_gateway().await;
    let client = MachineConnectionClient::new(
        client_config(url, "wrong-token"),
        MachineIdentity::generate(),
        Arc::new(EchoHandler),
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_ne!(client.state(), ConnectionState::Connected);
    assert!(
        task.is_finished(),
        "a rejected token must be terminal, state = {:?}",
        client.state()
    );
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn request_to_offline_machine_fails() {
    let (gateway, _auth, _url) = start_gateway().await;
    let error = gateway
        .registry()
        .request("missing", "echo", json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "machine_offline");
}

#[tokio::test]
async fn client_reconnects_after_server_restart() {
    let auth = Arc::new(MemoryAuth::default());
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    let (gateway, url, server) = start_gateway_with(auth.clone()).await;
    let address: std::net::SocketAddr = url
        .trim_start_matches("ws://")
        .trim_end_matches("/machine")
        .parse()
        .unwrap();
    let registry = gateway.registry();

    let client = MachineConnectionClient::new(
        client_config(url, "test-token"),
        identity,
        Arc::new(EchoHandler),
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_connected(&client).await;
    assert!(registry
        .request("m1", "echo", json!({"n": 1}))
        .await
        .is_ok());

    // Close the live socket and stop the listener: this simulates a server
    // restart (aborting only the accept loop would leave established
    // connections alive).
    registry.disconnect("m1");
    server.abort();
    let _ = server.await;
    for _ in 0..100 {
        if client.state() != ConnectionState::Connected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(client.state(), ConnectionState::Connected);

    // A fresh gateway on the same port re-authenticates the same identity.
    let (gateway2, server) = start_gateway_on(auth, address).await;
    let registry2 = gateway2.registry();
    let mut response = None;
    for _ in 0..100 {
        if registry2.is_online("m1") {
            if let Ok(value) = registry2.request("m1", "echo", json!({"n": 2})).await {
                response = Some(value);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let response = response.expect("reconnected client should serve requests");
    assert_eq!(response["method"], "echo");
    assert_eq!(response["payload"]["n"], 2);

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    server.abort();
}

#[tokio::test]
async fn rejects_identity_with_mismatched_pinned_key() {
    let (gateway, auth, url) = start_gateway().await;
    let registry = gateway.registry();
    let first_identity = MachineIdentity::generate();
    auth.register("m1", &first_identity.public_key_base64(), "test-token");

    let first = MachineConnectionClient::new(
        client_config(url.clone(), "test-token"),
        first_identity,
        Arc::new(EchoHandler),
    );
    let first_shutdown = CancellationToken::new();
    let first_task = tokio::spawn(first.clone().run(first_shutdown.clone()));
    wait_connected(&first).await;
    assert!(registry.is_online("m1"));

    // Same machine id, different key: the challenge signature cannot verify.
    let impostor = MachineConnectionClient::new(
        client_config(url, "test-token"),
        MachineIdentity::generate(),
        Arc::new(EchoHandler),
    );
    let impostor_shutdown = CancellationToken::new();
    let impostor_task = tokio::spawn(impostor.clone().run(impostor_shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_ne!(impostor.state(), ConnectionState::Connected);
    // The legitimate connection is untouched.
    assert!(registry.request("m1", "echo", json!({})).await.is_ok());

    first_shutdown.cancel();
    impostor_shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), first_task).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), impostor_task).await;
}

#[tokio::test]
async fn accepts_key_rotation_proven_by_the_previous_key() {
    let auth = Arc::new(MemoryAuth::default());
    let old_identity = MachineIdentity::generate();
    auth.register("m1", &old_identity.public_key_base64(), "test-token");
    let (gateway, url, _handle) = start_gateway_with(auth.clone()).await;
    let registry = gateway.registry();

    let first = MachineConnectionClient::new(
        client_config(url.clone(), "test-token"),
        old_identity.clone(),
        Arc::new(EchoHandler),
    );
    let first_shutdown = CancellationToken::new();
    let first_task = tokio::spawn(first.clone().run(first_shutdown.clone()));
    wait_connected(&first).await;
    assert_eq!(
        auth.keys.lock().unwrap().get("m1"),
        Some(&old_identity.public_key_base64())
    );

    first_shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), first_task).await;
    for _ in 0..100 {
        if !registry.is_online("m1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let new_identity = MachineIdentity::generate();
    let rotated = MachineConnectionClient::new(
        client_config(url, "test-token"),
        new_identity.clone(),
        Arc::new(EchoHandler),
    )
    .with_rotation_identity(old_identity);
    let rotated_shutdown = CancellationToken::new();
    let rotated_task = tokio::spawn(rotated.clone().run(rotated_shutdown.clone()));
    wait_connected(&rotated).await;
    assert_eq!(
        auth.keys.lock().unwrap().get("m1"),
        Some(&new_identity.public_key_base64())
    );

    rotated_shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), rotated_task).await;
}

#[tokio::test]
async fn rejects_key_rotation_signed_by_an_unrelated_key() {
    let auth = Arc::new(MemoryAuth::default());
    let first_identity = MachineIdentity::generate();
    auth.register("m1", &first_identity.public_key_base64(), "test-token");
    let (_gateway, url, _handle) = start_gateway_with(auth).await;

    let first = MachineConnectionClient::new(
        client_config(url.clone(), "test-token"),
        first_identity,
        Arc::new(EchoHandler),
    );
    let first_shutdown = CancellationToken::new();
    let first_task = tokio::spawn(first.clone().run(first_shutdown.clone()));
    wait_connected(&first).await;
    first_shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), first_task).await;

    let new_identity = MachineIdentity::generate();
    let attacker = MachineConnectionClient::new(
        client_config(url, "test-token"),
        new_identity,
        Arc::new(EchoHandler),
    )
    .with_rotation_identity(MachineIdentity::generate());
    let attacker_shutdown = CancellationToken::new();
    let attacker_task = tokio::spawn(attacker.clone().run(attacker_shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_ne!(attacker.state(), ConnectionState::Connected);

    attacker_shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), attacker_task).await;
}

#[tokio::test]
async fn failed_first_handshake_does_not_pin_the_presented_key() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    let code_id = insert_code(&auth, code);
    let (_gateway, url, _handle) = start_gateway_with(auth.clone()).await;

    // No Authorization header: this is a pairing-mode connection.
    let request = url.into_client_request().expect("client request");
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");
    let identity = MachineIdentity::generate();
    let hello = HelloPayload {
        machine_id: "m_new".to_string(),
        name: "poison".to_string(),
        hostname: "host".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        daemon_version: "test".to_string(),
        protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
        public_key: identity.public_key_base64(),
        pairing_code: Some(code.to_string()),
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            ConnectionEnvelope::request(methods::HELLO, json!(hello))
                .to_json()
                .into(),
        ))
        .await
        .expect("hello");

    let challenge = socket.next().await.expect("challenge").expect("frame");
    let challenge: ConnectionEnvelope =
        serde_json::from_str(challenge.to_text().expect("text")).expect("envelope");
    assert_eq!(challenge.message_type, MessageType::Request);
    let challenge: fleet_core_protocol::connection::AuthChallenge =
        serde_json::from_value(challenge.payload).expect("challenge payload");

    let response = AuthResponse {
        machine_id: "m_new".to_string(),
        challenge_id: challenge.challenge_id,
        // Validly encoded signature over the wrong bytes.
        signature: identity.sign(b"not the challenge nonce"),
        rotation: None,
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            ConnectionEnvelope::request(methods::AUTH_RESPONSE, json!(response))
                .to_json()
                .into(),
        ))
        .await
        .expect("auth");

    // The server must report the failure as an error frame rather than a bare
    // socket close, so the client can stop instead of reconnecting forever.
    let frame = socket.next().await.expect("error frame").expect("frame");
    let envelope: ConnectionEnvelope =
        serde_json::from_str(frame.to_text().expect("text")).expect("envelope");
    assert_eq!(envelope.message_type, MessageType::Error);
    let error: ApiError = serde_json::from_value(envelope.payload).expect("api error");
    assert_eq!(error.code, error_codes::PAIRING_CODE_INVALID);

    assert_eq!(
        auth.keys.lock().unwrap().get("m_new"),
        None,
        "a failed handshake must not pin a key"
    );
    assert!(
        auth.tokens.lock().unwrap().is_empty(),
        "a failed handshake must not issue a token"
    );
    assert_eq!(
        auth.failures.lock().unwrap().get(&code_id),
        Some(&1),
        "the failed signature must count against the code"
    );
    let hash = fleet_core_auth::hash_pair_code(code).unwrap();
    assert!(
        auth.codes.lock().unwrap().contains_key(&hash),
        "the code must not be consumed"
    );
    assert!(!auth.consumed.lock().unwrap().contains(&hash));
}

#[tokio::test]
async fn pairing_code_issues_a_token_and_token_connects() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    let (gateway, url, _handle) = start_gateway_with(auth.clone()).await;

    let identity = MachineIdentity::generate();
    let result = fleet_core_connection::pair_machine(
        fleet_core_connection::PairingConfig {
            server_url: url.clone(),
            machine_id: "m_pair".into(),
            machine_name: "paired".into(),
            hostname: "test".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            daemon_version: "0.1.0".into(),
            protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
            pairing_code: code.into(),
            handshake_timeout: Duration::from_secs(5),
        },
        &identity,
        None,
    )
    .await
    .unwrap();
    assert!(result.token.starts_with("flm_"));
    assert_eq!(result.machine_id, "m_pair");
    assert_eq!(result.org_id.as_deref(), Some("org_default"));
    // Pairing persisted the key and the token hash.
    assert_eq!(
        auth.keys.lock().unwrap().get("m_pair"),
        Some(&identity.public_key_base64())
    );
    assert_eq!(
        auth.tokens
            .lock()
            .unwrap()
            .get(&fleet_core_auth::hash_token(&result.token)),
        Some(&"m_pair".to_string())
    );

    // The one-shot pairing socket closes immediately; the paired machine must
    // be able to reconnect with the token it just received.
    let mut config = client_config(url, &result.token);
    config.machine_id = "m_pair".into();
    config.machine_name = "paired".into();
    let client = MachineConnectionClient::new(config, identity, Arc::new(EchoHandler));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_connected(&client).await;
    assert!(gateway.registry().is_online("m_pair"));
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn unpaired_machine_without_credentials_is_terminal() {
    let auth = Arc::new(MemoryAuth::default());
    let (_gateway, url, _handle) = start_gateway_with(auth).await;
    let identity = MachineIdentity::generate();

    // No Authorization header and no pairing code: no pinned key exists.
    let error = raw_hello_error(&url, test_hello("m1", &identity.public_key_base64(), None)).await;
    assert_eq!(error.code, error_codes::MACHINE_NOT_PAIRED);

    let client =
        MachineConnectionClient::new(client_config(url, ""), identity, Arc::new(EchoHandler));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_finished(&task, "unpaired client").await;
    assert_ne!(client.state(), ConnectionState::Connected);
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn paired_machine_without_token_is_terminal() {
    let auth = Arc::new(MemoryAuth::default());
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    let (_gateway, url, _handle) = start_gateway_with(auth).await;

    // A pinned machine must present its per-machine token.
    let error = raw_hello_error(&url, test_hello("m1", &identity.public_key_base64(), None)).await;
    assert_eq!(error.code, error_codes::MACHINE_TOKEN_REQUIRED);

    let client =
        MachineConnectionClient::new(client_config(url, ""), identity, Arc::new(EchoHandler));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_finished(&task, "token-required client").await;
    assert_ne!(client.state(), ConnectionState::Connected);
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn pair_machine_rejects_unknown_code() {
    let auth = Arc::new(MemoryAuth::default());
    let (_gateway, url, _handle) = start_gateway_with(auth).await;
    let identity = MachineIdentity::generate();

    let error = fleet_core_connection::pair_machine(
        pairing_config(url, "m_unknown", "0000-0000-0000"),
        &identity,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, GatewayError::PairingCodeInvalid(_)),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn re_pairing_rotates_token_and_old_token_is_rejected() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    let (gateway, url, _handle) = start_gateway_with(auth.clone()).await;
    let identity = MachineIdentity::generate();

    let first = fleet_core_connection::pair_machine(
        pairing_config(url.clone(), "m_pair", code),
        &identity,
        None,
    )
    .await
    .unwrap();
    // Re-pairing requires a fresh code: the first one is single-use.
    let code2 = "MNOP-QRST-VWXY";
    insert_code(&auth, code2);
    let second = fleet_core_connection::pair_machine(
        pairing_config(url.clone(), "m_pair", code2),
        &identity,
        None,
    )
    .await
    .unwrap();
    assert_ne!(
        first.token, second.token,
        "re-pairing must rotate the machine token"
    );
    assert!(
        auth.machine_for_token(&first.token)
            .await
            .unwrap()
            .is_none(),
        "the previous token must stop resolving"
    );
    assert_eq!(
        auth.machine_for_token(&second.token)
            .await
            .unwrap()
            .as_deref(),
        Some("m_pair")
    );

    // The old token is rejected at the upgrade and the client stops.
    let mut old_config = client_config(url.clone(), &first.token);
    old_config.machine_id = "m_pair".into();
    let old_client =
        MachineConnectionClient::new(old_config, identity.clone(), Arc::new(EchoHandler));
    let old_shutdown = CancellationToken::new();
    let old_task = tokio::spawn(old_client.clone().run(old_shutdown.clone()));
    wait_finished(&old_task, "old token client").await;
    assert_ne!(old_client.state(), ConnectionState::Connected);
    old_shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), old_task).await;

    // The rotated token connects.
    let mut new_config = client_config(url, &second.token);
    new_config.machine_id = "m_pair".into();
    let new_client = MachineConnectionClient::new(new_config, identity, Arc::new(EchoHandler));
    let new_shutdown = CancellationToken::new();
    let new_task = tokio::spawn(new_client.clone().run(new_shutdown.clone()));
    wait_connected(&new_client).await;
    assert!(gateway.registry().is_online("m_pair"));
    new_shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), new_task).await;
}

#[tokio::test]
async fn pairing_error_frame_reaches_the_client() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    *auth.complete_pairing_error.lock().unwrap() = Some(ApiError::new(
        error_codes::MACHINE_KEY_MISMATCH,
        "presented key does not match",
    ));
    let (_gateway, url, _handle) = start_gateway_with(auth).await;
    let identity = MachineIdentity::generate();

    let error =
        fleet_core_connection::pair_machine(pairing_config(url, "m_pair", code), &identity, None)
            .await
            .unwrap_err();
    // The storage error must arrive as a typed error frame, not as a socket
    // close (which would map to `Protocol`).
    assert!(
        matches!(error, GatewayError::MachineKeyMismatch(_)),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn empty_pairing_code_is_treated_as_absent() {
    let auth = Arc::new(MemoryAuth::default());
    let (_gateway, url, _handle) = start_gateway_with(auth).await;
    let identity = MachineIdentity::generate();

    for code in ["", "   ", "\n\t"] {
        let error = raw_hello_error(
            &url,
            test_hello("m1", &identity.public_key_base64(), Some(code.into())),
        )
        .await;
        // No credential and no usable code: this is the unpaired case, not a
        // bogus pairing attempt.
        assert_eq!(error.code, error_codes::MACHINE_NOT_PAIRED, "code {code:?}");
    }
}

#[tokio::test]
async fn pair_machine_with_a_mismatched_key_is_a_terminal_key_mismatch() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    let pinned = MachineIdentity::generate();
    auth.register("m_pair", &pinned.public_key_base64(), "token-a");
    let (_gateway, url, _handle) = start_gateway_with(auth.clone()).await;

    let replacement = MachineIdentity::generate();
    let error = fleet_core_connection::pair_machine(
        pairing_config(url, "m_pair", code),
        &replacement,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, GatewayError::MachineKeyMismatch(_)),
        "expected machine_key_mismatch, got {error}"
    );
    assert_eq!(
        auth.keys.lock().unwrap().get("m_pair"),
        Some(&pinned.public_key_base64()),
        "a rejected pairing must not re-pin the key"
    );
}

#[tokio::test]
async fn token_for_another_machine_id_is_rejected() {
    let auth = Arc::new(MemoryAuth::default());
    let identity_a = MachineIdentity::generate();
    let identity_b = MachineIdentity::generate();
    auth.register("m1", &identity_a.public_key_base64(), "token-a");
    auth.register("m2", &identity_b.public_key_base64(), "token-b");
    let (gateway, url, _handle) = start_gateway_with(auth.clone()).await;

    let error = raw_upgrade_error(
        &url,
        Some("token-a"),
        test_hello("m2", &identity_b.public_key_base64(), None),
    )
    .await;
    assert_eq!(error.code, error_codes::MACHINE_KEY_MISMATCH);
    assert!(!gateway.registry().is_online("m2"));
    assert_eq!(
        auth.keys.lock().unwrap().get("m2"),
        Some(&identity_b.public_key_base64())
    );
    assert_eq!(
        auth.machine_for_token("token-b").await.unwrap().as_deref(),
        Some("m2"),
        "the legitimate token must still resolve"
    );
}

#[tokio::test]
async fn pairing_hello_with_token_for_another_machine_is_rejected() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    let identity_a = MachineIdentity::generate();
    let identity_b = MachineIdentity::generate();
    auth.register("m1", &identity_a.public_key_base64(), "token-a");
    let (_gateway, url, _handle) = start_gateway_with(auth.clone()).await;

    let error = raw_upgrade_error(
        &url,
        Some("token-a"),
        test_hello("m2", &identity_b.public_key_base64(), Some(code.into())),
    )
    .await;
    assert_eq!(error.code, error_codes::MACHINE_KEY_MISMATCH);
    assert!(auth.keys.lock().unwrap().get("m2").is_none());
    let hash = fleet_core_auth::hash_pair_code(code).unwrap();
    assert!(!auth.consumed.lock().unwrap().contains(&hash));
}

#[tokio::test]
async fn wrong_challenge_id_is_a_terminal_key_mismatch() {
    let auth = Arc::new(MemoryAuth::default());
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    let (_gateway, url, _handle) = start_gateway_with(auth).await;

    let mut request = url.into_client_request().expect("client request");
    request.headers_mut().insert(
        "Authorization",
        "Bearer test-token".parse().expect("header"),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");
    let hello = test_hello("m1", &identity.public_key_base64(), None);
    socket
        .send(Message::Text(
            ConnectionEnvelope::request(methods::HELLO, json!(hello))
                .to_json()
                .into(),
        ))
        .await
        .expect("hello");

    let challenge = socket.next().await.expect("challenge").expect("frame");
    let challenge: ConnectionEnvelope =
        serde_json::from_str(challenge.to_text().expect("text")).expect("envelope");
    assert_eq!(challenge.message_type, MessageType::Request);

    let response = AuthResponse {
        machine_id: "m1".to_string(),
        challenge_id: "not-the-issued-challenge".to_string(),
        signature: identity.sign(b"anything"),
        rotation: None,
    };
    socket
        .send(Message::Text(
            ConnectionEnvelope::request(methods::AUTH_RESPONSE, json!(response))
                .to_json()
                .into(),
        ))
        .await
        .expect("auth");

    let frame = socket.next().await.expect("error frame").expect("frame");
    let envelope: ConnectionEnvelope =
        serde_json::from_str(frame.to_text().expect("text")).expect("envelope");
    assert_eq!(envelope.message_type, MessageType::Error);
    let error: ApiError = serde_json::from_value(envelope.payload).expect("api error");
    assert_eq!(error.code, error_codes::MACHINE_KEY_MISMATCH);
}

async fn upgrade_status(
    url: &str,
    header: tokio_tungstenite::tungstenite::http::HeaderValue,
) -> u16 {
    let mut request = url.into_client_request().expect("client request");
    request.headers_mut().insert(
        tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
        header,
    );
    match tokio_tungstenite::connect_async(request).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Ok(_) => panic!("the upgrade unexpectedly succeeded"),
        Err(error) => panic!("unexpected upgrade error: {error}"),
    }
}

#[tokio::test]
async fn malformed_authorization_header_is_rejected_with_401() {
    let (_gateway, _auth, url) = start_gateway().await;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    for value in ["Basic abc", "Bearer", "Bearer   "] {
        assert_eq!(
            upgrade_status(&url, HeaderValue::from_str(value).unwrap()).await,
            401,
            "header {value:?} must be rejected"
        );
    }
    // Non-UTF-8 header bytes cannot be sent by the WebSocket client library,
    // so that path is covered by the unit test in `server.rs`.
}

#[tokio::test]
async fn wrong_signatures_lock_the_pairing_code() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    let code_id = insert_code(&auth, code);
    let (_gateway, url, _handle) = start_gateway_with(auth.clone()).await;

    for _ in 0..5 {
        bad_signature_attempt(&url, code, "m_bad").await;
    }
    assert_eq!(auth.failures.lock().unwrap().get(&code_id), Some(&5));
    assert!(auth.keys.lock().unwrap().get("m_bad").is_none());

    // The sixth attempt is rejected before the challenge: the code is locked.
    let identity = MachineIdentity::generate();
    let error = fleet_core_connection::pair_machine(
        pairing_config(url.clone(), "m_bad", code),
        &identity,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, GatewayError::PairingCodeInvalid(_)),
        "expected pairing_code_invalid, got {error}"
    );

    // A persistent client treats it as terminal.
    let mut config = client_config(url, "");
    config.machine_id = "m_bad".into();
    config.pairing_code = Some(code.into());
    let client = MachineConnectionClient::new(config, identity, Arc::new(EchoHandler));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_finished(&task, "locked-code client").await;
    assert_ne!(client.state(), ConnectionState::Connected);
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn pair_machine_rejects_used_and_expired_codes() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    let expired = "MNOP-QRST-VWXY";
    insert_expired_code(&auth, expired);
    let (_gateway, url, _handle) = start_gateway_with(auth).await;
    let identity = MachineIdentity::generate();

    // First redemption succeeds.
    fleet_core_connection::pair_machine(
        pairing_config(url.clone(), "m_used", code),
        &identity,
        None,
    )
    .await
    .unwrap();

    // Reusing the code is rejected.
    let error = fleet_core_connection::pair_machine(
        pairing_config(url.clone(), "m_used", code),
        &identity,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, GatewayError::PairingCodeInvalid(_)));

    // An expired code is rejected the same way.
    let error = fleet_core_connection::pair_machine(
        pairing_config(url, "m_expired", expired),
        &identity,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, GatewayError::PairingCodeInvalid(_)));
}

#[tokio::test]
async fn pin_key_error_frame_stops_the_client() {
    let auth = Arc::new(MemoryAuth::default());
    let old_identity = MachineIdentity::generate();
    auth.register("m1", &old_identity.public_key_base64(), "test-token");
    *auth.pin_key_error.lock().unwrap() = Some(ApiError::new(
        error_codes::MACHINE_KEY_MISMATCH,
        "pin rejected",
    ));
    let (_gateway, url, _handle) = start_gateway_with(auth).await;

    let new_identity = MachineIdentity::generate();
    let mut config = client_config(url, "test-token");
    config.machine_id = "m1".into();
    let client = MachineConnectionClient::new(config, new_identity, Arc::new(EchoHandler))
        .with_rotation_identity(old_identity);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_finished(&task, "pin-key error client").await;
    assert_ne!(client.state(), ConnectionState::Connected);
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn client_auto_pairs_and_persists_the_token() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    let (gateway, url, _handle) = start_gateway_with(auth.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("machine.key.token");

    let mut config = client_config(url, "");
    config.machine_id = "m_auto".into();
    config.machine_name = "auto".into();
    config.pairing_code = Some(code.into());
    config.token_file = Some(token_path.clone());
    let client =
        MachineConnectionClient::new(config, MachineIdentity::generate(), Arc::new(EchoHandler));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_connected(&client).await;

    let stored = std::fs::read_to_string(&token_path).expect("token file");
    assert!(stored.trim().starts_with("flm_"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let code_hash = fleet_core_auth::hash_pair_code(code).unwrap();
    assert!(auth.consumed.lock().unwrap().contains(&code_hash));

    // Force a reconnect. The in-memory code must be cleared, so the client
    // authenticates with the saved token rather than replaying a used code.
    gateway.registry().disconnect("m_auto");
    for _ in 0..100 {
        if client.state() != ConnectionState::Connected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_connected(&client).await;
    assert!(gateway.registry().is_online("m_auto"));

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn token_save_failure_is_terminal() {
    let auth = Arc::new(MemoryAuth::default());
    let code = "ABCD-EFGH-JKMN";
    insert_code(&auth, code);
    let (_gateway, url, _handle) = start_gateway_with(auth).await;
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("token-is-a-directory");
    std::fs::create_dir(&token_path).unwrap();

    let mut config = client_config(url, "");
    config.machine_id = "m_save".into();
    config.pairing_code = Some(code.into());
    config.token_file = Some(token_path);
    let client =
        MachineConnectionClient::new(config, MachineIdentity::generate(), Arc::new(EchoHandler));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_finished(&task, "token save failure client").await;
    assert_ne!(client.state(), ConnectionState::Connected);
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn a_stale_pairing_code_is_not_sent_with_a_valid_token() {
    let auth = Arc::new(MemoryAuth::default());
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    let (_gateway, url, _handle) = start_gateway_with(auth).await;

    let mut config = client_config(url, "test-token");
    config.machine_id = "m1".into();
    // A lingering single-use code from the provisioning config.
    config.pairing_code = Some("0000-0000-0000".into());
    let client = MachineConnectionClient::new(config, identity, Arc::new(EchoHandler));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_connected(&client).await;
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn unauthenticated_attempts_do_not_throttle_an_authenticated_reconnect() {
    let auth = Arc::new(MemoryAuth::default());
    let identity = MachineIdentity::generate();
    auth.register("m1", &identity.public_key_base64(), "test-token");
    let (_gateway, url, _handle) = start_gateway_with(auth).await;

    // Exhaust the pairing budget for the loopback peer.
    for _ in 0..30 {
        let request = url.as_str().into_client_request().expect("request");
        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("unauthenticated upgrade");
        drop(socket);
    }

    // An authenticated reconnect is not charged and must still succeed.
    let client = MachineConnectionClient::new(
        client_config(url, "test-token"),
        identity,
        Arc::new(EchoHandler),
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    wait_connected(&client).await;
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

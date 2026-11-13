use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_fleet_auth::{MachineIdentity, TokenAuth};
use agent_fleet_common::ApiError;
use agent_fleet_connection_gateway::{
    ClientConfig, ConnectionGateway, ConnectionState, DaemonRequestHandler, MachineAuthenticator,
    MachineConnectionClient,
};
use agent_fleet_protocol::connection::ConnectionAccepted;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MemoryAuth {
    keys: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl MachineAuthenticator for MemoryAuth {
    async fn pinned_key(&self, machine_id: &str) -> Result<Option<String>, ApiError> {
        Ok(self.keys.lock().unwrap().get(machine_id).cloned())
    }

    async fn pin_key(&self, machine_id: &str, public_key: &str) -> Result<(), ApiError> {
        self.keys
            .lock()
            .unwrap()
            .insert(machine_id.to_string(), public_key.to_string());
        Ok(())
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

async fn start_gateway() -> (Arc<ConnectionGateway>, String) {
    let (gateway, url, _handle) = start_gateway_with(Arc::new(MemoryAuth::default())).await;
    (gateway, url)
}

async fn start_gateway_with(
    auth: Arc<dyn MachineAuthenticator>,
) -> (Arc<ConnectionGateway>, String, tokio::task::JoinHandle<()>) {
    let gateway = ConnectionGateway::new(
        TokenAuth::new("test-token"),
        auth,
        Duration::from_millis(500),
        Duration::from_secs(5),
    );
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
    let gateway = ConnectionGateway::new(
        TokenAuth::new("test-token"),
        auth,
        Duration::from_millis(500),
        Duration::from_secs(5),
    );
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

#[tokio::test]
async fn machine_connects_and_routes_requests() {
    let (gateway, url) = start_gateway().await;
    let registry = gateway.registry();
    let client = MachineConnectionClient::new(
        client_config(url, "test-token"),
        MachineIdentity::generate(),
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
async fn rejects_bad_token() {
    let (_gateway, url) = start_gateway().await;
    let client = MachineConnectionClient::new(
        client_config(url, "wrong-token"),
        MachineIdentity::generate(),
        Arc::new(EchoHandler),
    );
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(client.clone().run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_ne!(client.state(), ConnectionState::Connected);
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn request_to_offline_machine_fails() {
    let (gateway, _url) = start_gateway().await;
    let error = gateway
        .registry()
        .request("missing", "echo", json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "machine_offline");
}

#[tokio::test]
async fn client_reconnects_after_server_restart() {
    let auth: Arc<dyn MachineAuthenticator> = Arc::new(MemoryAuth::default());
    let (gateway, url, server) = start_gateway_with(auth.clone()).await;
    let address: std::net::SocketAddr = url
        .trim_start_matches("ws://")
        .trim_end_matches("/machine")
        .parse()
        .unwrap();
    let registry = gateway.registry();

    let client = MachineConnectionClient::new(
        client_config(url, "test-token"),
        MachineIdentity::generate(),
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
    let (gateway, url) = start_gateway().await;
    let registry = gateway.registry();

    let first = MachineConnectionClient::new(
        client_config(url.clone(), "test-token"),
        MachineIdentity::generate(),
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

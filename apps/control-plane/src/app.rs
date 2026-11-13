//! Axum application assembly: API routes, machine WebSocket, CORS and the
//! optional single-page-app static fallback.

use axum::http::{header, Method};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::routes;
use crate::state::AppState;

/// Build the full control-plane router.
///
/// * `/api/v1/*` – REST plus the browser event WebSocket.
/// * `/ws/machine` – machine reverse connection (the gateway authenticates).
/// * everything else – the built web UI when `web.static_dir` exists.
pub fn build_router(state: AppState) -> Router {
    let api = routes::api_router(state.clone()).with_state(state.clone());
    let machine_ws = state.gateway.clone().router();

    let mut router = Router::new()
        .nest("/api/v1", api)
        .nest("/ws", machine_ws)
        .layer(cors_layer())
        .layer(TraceLayer::new_for_http());

    if let Some(directory) = state.config.web.static_dir.as_deref() {
        if directory.is_dir() {
            let index = directory.join("index.html");
            if index.is_file() {
                // Serve real assets and fall back to index.html so client-side
                // routes survive a page reload. Unmatched `/api/v1` paths are
                // handled by the API router's own JSON fallback.
                router = router
                    .fallback_service(ServeDir::new(directory).fallback(ServeFile::new(index)));
            }
        }
    }

    router
}

/// Permissive CORS for the local web development server.
fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use agent_fleet_auth::MachineIdentity;
    use agent_fleet_common::{error_codes, now, ApiError};
    use agent_fleet_config::ControlPlaneConfig;
    use agent_fleet_connection_gateway::{
        ClientConfig, DaemonRequestHandler, MachineConnectionClient,
    };
    use agent_fleet_protocol::agent::{
        AgentCapabilities, AgentSession, ProviderInfo, ProviderKind, SessionStatus,
    };
    use agent_fleet_protocol::connection::PlatformEvent;
    use agent_fleet_protocol::rpc::{methods, CreateAgentSessionRequest};
    use agent_fleet_protocol::workspace::{RegisterWorkspaceRequest, Workspace};
    use agent_fleet_storage::Database;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use futures::StreamExt;
    use serde_json::{json, Value};
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::*;

    async fn test_state() -> AppState {
        let db = Arc::new(Database::connect_in_memory().await.unwrap());
        let mut config = ControlPlaneConfig::default();
        config.auth.admin_token = "test-token".to_string();
        AppState::new(db, config).await.unwrap()
    }

    fn request(
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = match body {
            Some(value) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        builder.body(body).unwrap()
    }

    async fn call(
        state: &AppState,
        request: Request<Body>,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        let response = build_router(state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, headers, value)
    }

    #[tokio::test]
    async fn health_is_public() {
        let state = test_state().await;
        let (status, _, body) =
            call(&state, request(Method::GET, "/api/v1/health", None, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["machines_online"], 0);
        assert!(body["version"].as_str().is_some());
    }

    #[tokio::test]
    async fn machines_require_a_bearer_token() {
        let state = test_state().await;
        let (status, _, body) =
            call(&state, request(Method::GET, "/api/v1/machines", None, None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthorized");
    }

    #[tokio::test]
    async fn machines_list_with_token() {
        let state = test_state().await;
        let (status, _, body) = call(
            &state,
            request(Method::GET, "/api/v1/machines", Some("test-token"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!([]));
    }

    #[tokio::test]
    async fn unknown_machine_is_not_found() {
        let state = test_state().await;
        let (status, _, body) = call(
            &state,
            request(
                Method::GET,
                "/api/v1/machines/missing",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
    }

    #[tokio::test]
    async fn creating_a_session_on_an_offline_machine_is_503() {
        let state = test_state().await;
        let body = json!({
            "machine_id": "missing",
            "provider": "mock",
            "cwd": "/tmp",
        });
        let (status, _, body) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/sessions",
                Some("test-token"),
                Some(body),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "machine_offline");
    }

    #[tokio::test]
    async fn deleting_an_unknown_workspace_is_not_found() {
        let state = test_state().await;
        let (status, _, body) = call(
            &state,
            request(
                Method::DELETE,
                "/api/v1/workspaces/missing",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
    }

    #[tokio::test]
    async fn browser_event_socket_requires_a_token() {
        let state = test_state().await;
        let app = build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let result =
            tokio_tungstenite::connect_async(format!("ws://{address}/api/v1/events")).await;
        assert!(result.is_err(), "unauthenticated upgrade must fail");
        server.abort();
    }

    #[tokio::test]
    async fn browser_event_socket_streams_platform_events() {
        let state = test_state().await;
        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (mut socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{address}/api/v1/events?token=test-token"
        ))
        .await
        .expect("event socket should upgrade");

        let mut event = PlatformEvent::new("agent.event", json!({"text": "hello"}));
        event.machine_id = Some("m1".to_string());
        event.session_id = Some("session_1".to_string());

        let mut received: Option<PlatformEvent> = None;
        for _ in 0..20 {
            state.events.publish(event.clone());
            match tokio::time::timeout(Duration::from_millis(100), socket.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                    received = Some(serde_json::from_str(&text).unwrap());
                    break;
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(err))) => panic!("websocket error: {err}"),
                Ok(None) => panic!("websocket closed unexpectedly"),
                Err(_) => continue,
            }
        }

        let received = received.expect("platform event should be delivered");
        assert_eq!(received.event_id, event.event_id);
        assert_eq!(received.event_type, "agent.event");
        assert_eq!(received.machine_id.as_deref(), Some("m1"));
        assert_eq!(received.session_id.as_deref(), Some("session_1"));
        assert_eq!(received.payload, json!({"text": "hello"}));

        server.abort();
    }

    /// Minimal daemon used by the loopback end-to-end test.
    struct FakeDaemon {
        sessions: Mutex<Vec<AgentSession>>,
    }

    impl FakeDaemon {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl DaemonRequestHandler for FakeDaemon {
        async fn handle(&self, method: &str, payload: Value) -> Result<Value, ApiError> {
            match method {
                methods::PROVIDERS_LIST => {
                    let providers = vec![ProviderInfo {
                        id: "mock".into(),
                        kind: ProviderKind::Mock,
                        display_name: "Mock".into(),
                        version: Some("1".into()),
                        available: true,
                        capabilities: AgentCapabilities::full(),
                        models: vec!["default".into()],
                    }];
                    serde_json::to_value(providers)
                        .map_err(|err| ApiError::internal(err.to_string()))
                }
                methods::WORKSPACES_LIST => Ok(json!([])),
                methods::WORKSPACES_REGISTER => {
                    let request: RegisterWorkspaceRequest = serde_json::from_value(payload)
                        .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))?;
                    let timestamp = now();
                    let workspace = Workspace {
                        id: "ws_test".into(),
                        machine_id: "m_test".into(),
                        project_id: request.project_id,
                        name: request.name.unwrap_or_else(|| "test-workspace".into()),
                        root_path: request.root_path,
                        is_git_repository: false,
                        created_at: timestamp,
                        updated_at: timestamp,
                    };
                    serde_json::to_value(workspace)
                        .map_err(|err| ApiError::internal(err.to_string()))
                }
                methods::WORKSPACES_REMOVE => Ok(json!({"ok": true})),
                methods::SESSIONS_CREATE => {
                    let request: CreateAgentSessionRequest = serde_json::from_value(payload)
                        .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))?;
                    let provider = request.provider.unwrap_or_else(|| "mock".into());
                    let cwd = request.cwd.unwrap_or_else(|| "/tmp".into());
                    let mut session = AgentSession::new("m_test", &provider, "mock", cwd);
                    session.workspace_id = request.workspace_id;
                    session.title = request.title;
                    session.status = SessionStatus::Running;
                    self.sessions.lock().await.push(session.clone());
                    serde_json::to_value(session).map_err(|err| ApiError::internal(err.to_string()))
                }
                methods::SESSIONS_LIST => {
                    let sessions = self.sessions.lock().await.clone();
                    serde_json::to_value(sessions)
                        .map_err(|err| ApiError::internal(err.to_string()))
                }
                other => Err(ApiError::new(
                    error_codes::UNSUPPORTED,
                    format!("unsupported method `{other}`"),
                )),
            }
        }
    }

    #[tokio::test]
    async fn spa_fallback_serves_index_for_client_routes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html>spa</html>").unwrap();

        let db = Arc::new(Database::connect_in_memory().await.unwrap());
        let mut config = ControlPlaneConfig::default();
        config.auth.admin_token = "test-token".to_string();
        config.web.static_dir = Some(dir.path().to_path_buf());
        let state = AppState::new(db, config).await.unwrap();

        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/machines/example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>spa</html>");
    }

    #[tokio::test]
    async fn loopback_machine_online_flow() {
        let state = test_state().await;
        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let event_task = crate::services::event_service::spawn(state.clone());

        let client_config = ClientConfig::new(
            format!("ws://{address}/ws/machine"),
            "test-token",
            "m_test",
            "Test Machine",
        );
        let client = MachineConnectionClient::new(
            client_config,
            MachineIdentity::generate(),
            Arc::new(FakeDaemon::new()),
        );
        let shutdown = CancellationToken::new();
        let client_task = {
            let client = client.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                client.run(shutdown).await;
            })
        };

        // Wait for the gateway event pipeline to register the machine.
        let mut machines = Value::Null;
        for _ in 0..100 {
            let (status, _, body) = call(
                &state,
                request(Method::GET, "/api/v1/machines", Some("test-token"), None),
            )
            .await;
            if status == StatusCode::OK
                && body.as_array().map(|items| items.len()).unwrap_or(0) == 1
            {
                machines = body;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let machines = machines
            .as_array()
            .expect("machine should be registered within five seconds");
        assert_eq!(machines[0]["id"], "m_test");
        assert_eq!(machines[0]["status"], "online");
        assert_eq!(machines[0]["providers"][0]["id"], "mock");

        // Provider lookup is proxied while the machine is online.
        let (status, _, providers) = call(
            &state,
            request(
                Method::GET,
                "/api/v1/machines/m_test/providers",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(providers[0]["id"], "mock");

        // Registering a workspace refreshes the machine snapshot counters.
        let (status, _, workspace) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/workspaces",
                Some("test-token"),
                Some(json!({
                    "machine_id": "m_test",
                    "root_path": "/tmp",
                    "name": "test-workspace",
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(workspace["id"], "ws_test");
        let (status, _, machine) = call(
            &state,
            request(
                Method::GET,
                "/api/v1/machines/m_test",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(machine["workspace_count"], 1);
        assert_eq!(machine["workspaces"][0]["id"], "ws_test");

        // Session creation is proxied to the fake daemon.
        let (status, _, session) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/sessions",
                Some("test-token"),
                Some(json!({
                    "machine_id": "m_test",
                    "provider": "mock",
                    "cwd": "/tmp",
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let session_id = session["id"]
            .as_str()
            .expect("daemon response should contain a session id")
            .to_string();
        assert_eq!(session["machine_id"], "m_test");

        // The live session list from the daemon is merged into persisted rows.
        let (status, _, sessions) = call(
            &state,
            request(Method::GET, "/api/v1/sessions", Some("test-token"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(sessions
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == session_id.as_str()));

        shutdown.cancel();
        client_task.abort();
        event_task.abort();
        server.abort();
    }
}

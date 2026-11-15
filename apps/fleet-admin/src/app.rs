//! Axum application assembly: API routes, machine WebSocket, CORS and the
//! optional single-page-app static fallback.

use axum::http::{header, HeaderName, HeaderValue, Method};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::routes;
use crate::state::AppState;

/// Build the full agent-admin router.
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
        .layer(axum::middleware::from_fn(security_headers))
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

/// Add headers that are safe for both the API and the SPA.
///
/// A strict CSP is intentionally not set here: Monaco loads language workers
/// from blob URLs, so a CSP needs to be designed with the final asset policy.
async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}

/// Permissive CORS for the local web development server.
fn cors_layer() -> CorsLayer {
    // The API is normally same-origin behind the SPA fallback. Only the local
    // Vite development server is allowed to make cross-origin requests.
    let dev_origins = [
        HeaderValue::from_static("http://localhost:5173"),
        HeaderValue::from_static("http://127.0.0.1:5173"),
    ];
    CorsLayer::new()
        .allow_origin(dev_origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("x-request-id"),
            HeaderName::from_static("x-agent-fleet-git-token"),
        ])
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use fleet_admin_storage::AdminStorage;
    use fleet_core_auth::MachineIdentity;
    use fleet_core_common::{error_codes, now, ApiError};
    use fleet_core_config::AdminConfig;
    use fleet_core_connection::{ClientConfig, DaemonRequestHandler, MachineConnectionClient};
    use fleet_core_protocol::agent::{
        AgentCapabilities, AgentMessage, AgentSession, AgentToolCall, AgentTurn, MessageRole,
        ProviderInfo, ProviderKind, SessionStatus, ToolCallStatus,
    };
    use fleet_core_protocol::connection::PlatformEvent;
    use fleet_core_protocol::git::{GitCheckoutResult, GitCommitResult};
    use fleet_core_protocol::rpc::{
        methods, CreateAgentSessionRequest, SessionDetail, SessionRequest, WorkspaceRequest,
    };
    use fleet_core_protocol::workspace::{RegisterWorkspaceRequest, Workspace};
    use futures::StreamExt;
    use serde_json::{json, Value};
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::*;

    fn test_config() -> AdminConfig {
        let mut config = AdminConfig::default();
        config.auth.machine_token = Some("test-machine-token".to_string());
        config
    }

    async fn test_state() -> AppState {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        AppState::new(db, config).await.unwrap()
    }

    #[tokio::test]
    async fn responses_include_baseline_security_headers() {
        let state = test_state().await;
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
        assert_eq!(
            response.headers().get(header::X_FRAME_OPTIONS).unwrap(),
            "DENY"
        );
        assert_eq!(
            response.headers().get(header::REFERRER_POLICY).unwrap(),
            "no-referrer"
        );
    }

    #[tokio::test]
    async fn active_sessions_of_offline_machines_become_interrupted() {
        let state = test_state().await;
        let mut session = AgentSession::new("m_offline", "mock", "mock-agent", "/tmp");
        session.status = SessionStatus::Running;
        state.db.sessions.upsert(&session).await.unwrap();

        let sessions = crate::services::agent_service::AgentService::list_sessions(&state, true)
            .await
            .unwrap();
        let session = sessions
            .into_iter()
            .find(|item| item.id == session.id)
            .expect("session");
        assert_eq!(session.status, SessionStatus::Interrupted);
        assert_eq!(session.metadata["machine_offline"], true);
    }

    #[tokio::test]
    async fn unrevoke_restores_the_pinned_key_and_organization() {
        let state = test_state().await;
        let timestamp = now();
        let machine = fleet_core_protocol::machine::Machine {
            id: "m_unrevoke".to_string(),
            name: "unrevoke".to_string(),
            hostname: "host".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            daemon_version: "0.1.0".to_string(),
            protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
            status: fleet_core_protocol::machine::MachineStatus::Offline,
            created_at: timestamp,
            updated_at: timestamp,
            connected_at: None,
            last_seen_at: None,
            providers: Vec::new(),
            workspaces: Vec::new(),
            workspace_count: 0,
            public_key: Some("pinned-key".to_string()),
        };
        state.db.machines.upsert(&machine).await.unwrap();
        state
            .db
            .machine_orgs
            .assign("m_unrevoke", fleet_core_protocol::authz::DEFAULT_ORG_ID)
            .await
            .unwrap();

        crate::services::machine_service::MachineService::remove(&state, "m_unrevoke")
            .await
            .unwrap();
        crate::services::machine_service::MachineService::unrevoke(&state, "m_unrevoke", None)
            .await
            .unwrap();

        let restored = state.db.machines.get("m_unrevoke").await.unwrap().unwrap();
        assert_eq!(restored.public_key.as_deref(), Some("pinned-key"));
        assert_eq!(
            state.db.machine_orgs.get("m_unrevoke").await.unwrap(),
            Some(fleet_core_protocol::authz::DEFAULT_ORG_ID.to_string())
        );
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
                methods::WORKSPACES_GET => {
                    let request: WorkspaceRequest = serde_json::from_value(payload)
                        .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))?;
                    let timestamp = now();
                    let workspace = Workspace {
                        id: request.workspace_id,
                        machine_id: "m_test".into(),
                        project_id: None,
                        name: "test-workspace".into(),
                        root_path: "/tmp".into(),
                        is_git_repository: true,
                        created_at: timestamp,
                        updated_at: timestamp,
                    };
                    serde_json::to_value(workspace)
                        .map_err(|err| ApiError::internal(err.to_string()))
                }
                methods::GIT_STAGE | methods::GIT_UNSTAGE => Ok(json!({"ok": true})),
                methods::GIT_COMMIT => {
                    let request: fleet_core_protocol::rpc::GitCommitRpcRequest =
                        serde_json::from_value(payload)
                            .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))?;
                    let summary = request
                        .commit
                        .message
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    serde_json::to_value(GitCommitResult {
                        commit: "deadbeef".into(),
                        branch: "main".into(),
                        summary,
                    })
                    .map_err(|err| ApiError::internal(err.to_string()))
                }
                methods::GIT_CHECKOUT => {
                    let request: fleet_core_protocol::rpc::GitCheckoutRpcRequest =
                        serde_json::from_value(payload)
                            .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))?;
                    serde_json::to_value(GitCheckoutResult {
                        branch: request.checkout.branch,
                    })
                    .map_err(|err| ApiError::internal(err.to_string()))
                }
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
                methods::SESSIONS_GET => {
                    let request: SessionRequest = serde_json::from_value(payload)
                        .map_err(|err| ApiError::invalid(format!("invalid payload: {err}")))?;
                    let sessions = self.sessions.lock().await;
                    let session = sessions
                        .iter()
                        .find(|session| session.id == request.session_id)
                        .cloned()
                        .ok_or_else(|| ApiError::not_found("session"))?;
                    let timestamp = now();
                    let detail = SessionDetail {
                        session: session.clone(),
                        turns: vec![AgentTurn {
                            id: "turn_test".into(),
                            session_id: session.id.clone(),
                            status: fleet_core_protocol::agent::TurnStatus::Completed,
                            started_at: timestamp,
                            completed_at: Some(timestamp),
                            error: None,
                        }],
                        messages: vec![AgentMessage {
                            id: "message_test".into(),
                            session_id: session.id.clone(),
                            turn_id: Some("turn_test".into()),
                            role: MessageRole::User,
                            content: "history from daemon".into(),
                            created_at: timestamp,
                        }],
                        tool_calls: vec![AgentToolCall {
                            id: "tool_test".into(),
                            session_id: session.id.clone(),
                            turn_id: Some("turn_test".into()),
                            name: "Bash".into(),
                            status: ToolCallStatus::Completed,
                            input: json!({"command": "echo hello"}),
                            output: json!({"stdout": "hello\n"}),
                            error: None,
                            started_at: timestamp,
                            completed_at: Some(timestamp),
                        }],
                        permissions: vec![],
                    };
                    serde_json::to_value(detail).map_err(|err| ApiError::internal(err.to_string()))
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

        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let mut config = test_config();
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
    async fn machine_websocket_uses_the_machine_token_not_the_admin_token() {
        let mut config = test_config();
        config.auth.admin_token = "admin-token".to_string();
        config.auth.machine_token = Some("machine-token".to_string());
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let state = AppState::new(db, config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = build_router(state.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let admin_client = MachineConnectionClient::new(
            ClientConfig::new(
                format!("ws://{address}/ws/machine"),
                "admin-token",
                "m_admin",
                "Admin Token",
            ),
            MachineIdentity::generate(),
            Arc::new(FakeDaemon::new()),
        );
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(admin_client.clone().run(shutdown.clone()));
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            !state.connections.is_online("m_admin"),
            "the REST admin token must not authenticate a machine"
        );
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;

        let machine_client = MachineConnectionClient::new(
            ClientConfig::new(
                format!("ws://{address}/ws/machine"),
                "machine-token",
                "m_machine",
                "Machine Token",
            ),
            MachineIdentity::generate(),
            Arc::new(FakeDaemon::new()),
        );
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(machine_client.clone().run(shutdown.clone()));
        for _ in 0..100 {
            if state.connections.is_online("m_machine") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(state.connections.is_online("m_machine"));
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        server.abort();
    }

    #[tokio::test]
    async fn browser_event_socket_accepts_user_tokens_and_filters_by_organization() {
        let state = test_state().await;
        let other_org = state.db.organizations.create("Other").await.unwrap();
        state
            .db
            .machine_orgs
            .assign("m_default", "org_default")
            .await
            .unwrap();
        state
            .db
            .machine_orgs
            .assign("m_other", &other_org.id)
            .await
            .unwrap();
        let token = fleet_core_auth::generate_user_token();
        state
            .db
            .users
            .create(
                "org_default",
                "Ada",
                fleet_core_protocol::authz::UserRole::Member,
                &fleet_core_auth::hash_token(&token),
            )
            .await
            .unwrap();

        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://{address}/api/v1/events?token={token}"))
                .await
                .expect("user token should upgrade the event socket");

        let mut own = PlatformEvent::new("agent.event", json!({"n": 1}));
        own.event_id = "evt_own".into();
        own.machine_id = Some("m_default".into());
        let mut foreign = PlatformEvent::new("agent.event", json!({"n": 2}));
        foreign.event_id = "evt_foreign".into();
        foreign.machine_id = Some("m_other".into());

        let mut received = Vec::new();
        for _ in 0..30 {
            state.events.publish(own.clone());
            state.events.publish(foreign.clone());
            match tokio::time::timeout(Duration::from_millis(100), socket.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                    let event: PlatformEvent = serde_json::from_str(&text).unwrap();
                    received.push(event.event_id);
                    if received.len() >= 3 {
                        break;
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(err))) => panic!("websocket error: {err}"),
                Ok(None) => panic!("websocket closed unexpectedly"),
                Err(_) => continue,
            }
        }
        assert!(
            !received.is_empty(),
            "own organization events should arrive"
        );
        assert!(
            !received.contains(&"evt_foreign".to_string()),
            "events from another organization must be filtered: {received:?}"
        );

        // An unknown token is rejected before the upgrade.
        let rejected =
            tokio_tungstenite::connect_async(format!("ws://{address}/api/v1/events?token=bogus"))
                .await;
        assert!(rejected.is_err());

        server.abort();
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
            "test-machine-token",
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
                && body
                    .as_array()
                    .map(|items| {
                        items.len() == 1
                            && items[0]["status"] == "online"
                            && items[0]["providers"][0]["id"] == "mock"
                    })
                    .unwrap_or(false)
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

        // Forcing a reconnect drops the live socket, then the daemon comes back.
        let (status, _, body) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/machines/m_test/reconnect",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert!(!state.connections.is_online("m_test"));
        let mut reconnected = false;
        for _ in 0..100 {
            if state.connections.is_online("m_test") {
                reconnected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            reconnected,
            "daemon did not reconnect after forced disconnect"
        );

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

    #[tokio::test]
    async fn session_history_is_proxied_from_the_daemon_without_an_admin_copy() {
        let (state, server, tasks) = start_fake_stack(test_config(), "test-token").await;
        let workspace_id = register_test_workspace(&state, "test-token").await;

        let (status, _, session) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/sessions",
                Some("test-token"),
                Some(json!({
                    "machine_id": "m_test",
                    "provider": "mock",
                    "workspace_id": workspace_id,
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let session_id = session["id"].as_str().unwrap().to_string();

        let (status, _, detail) = call(
            &state,
            request(
                Method::GET,
                &format!("/api/v1/sessions/{session_id}"),
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["messages"][0]["content"], "history from daemon");
        assert_eq!(detail["tool_calls"][0]["name"], "Bash");
        assert_eq!(detail["session"]["metadata"]["history_unavailable"], false);

        // The daemon owns the transcript; these tables are not part of the
        // admin schema at all.
        for table in ["agent_messages", "agent_tool_calls", "agent_turns"] {
            let (count,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            )
            .bind(table)
            .fetch_one(state.db.pool())
            .await
            .unwrap();
            assert_eq!(count, 0, "{table} must not exist in the admin database");
        }

        server.abort();
        for task in tasks {
            task.abort();
        }
    }

    #[tokio::test]
    async fn offline_session_history_is_not_faked_from_partial_cache() {
        let state = test_state().await;
        let session = AgentSession::new("m_offline", "mock", "mock", "/tmp");
        state.db.sessions.upsert(&session).await.unwrap();

        let detail = crate::services::agent_service::AgentService::get_session(&state, &session.id)
            .await
            .expect("offline detail");
        assert!(detail.messages.is_empty());
        assert!(detail.tool_calls.is_empty());
        assert_eq!(detail.session.status, SessionStatus::Interrupted);
        assert_eq!(detail.session.metadata["history_unavailable"], true);
    }

    async fn start_fake_stack(
        mut config: AdminConfig,
        token: &str,
    ) -> (
        AppState,
        tokio::task::JoinHandle<()>,
        Vec<tokio::task::JoinHandle<()>>,
    ) {
        config.auth.admin_token = token.to_string();
        let machine_token = config
            .auth
            .machine_token
            .clone()
            .expect("test machine token");
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let state = AppState::new(db, config).await.unwrap();
        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let event_task = crate::services::event_service::spawn(state.clone());

        let client = MachineConnectionClient::new(
            ClientConfig::new(
                format!("ws://{address}/ws/machine"),
                machine_token,
                "m_test",
                "Test Machine",
            ),
            MachineIdentity::generate(),
            Arc::new(FakeDaemon::new()),
        );
        let shutdown = CancellationToken::new();
        let client_task = tokio::spawn(async move {
            client.run(shutdown).await;
        });

        for _ in 0..100 {
            if state
                .machines
                .get("m_test")
                .await
                .ok()
                .flatten()
                .is_some_and(|machine| {
                    machine.status == fleet_core_protocol::machine::MachineStatus::Online
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        (state, server, vec![client_task, event_task])
    }

    async fn register_test_workspace(state: &AppState, token: &str) -> String {
        let (status, _, workspace) = call(
            state,
            request(
                Method::POST,
                "/api/v1/workspaces",
                Some(token),
                Some(json!({
                    "machine_id": "m_test",
                    "root_path": "/tmp",
                    "name": "test-workspace",
                })),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "workspace registration: {workspace}"
        );
        workspace["id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn git_writes_require_the_dedicated_token_and_record_audit_entries() {
        let mut config = test_config();
        config.git.write_token = Some("git-secret".to_string());
        let (state, server, _tasks) = start_fake_stack(config, "test-token").await;
        let workspace_id = register_test_workspace(&state, "test-token").await;
        let uri = format!("/api/v1/workspaces/{workspace_id}/git/stage");
        let body = json!({"paths": ["a.txt"]});

        let (status, _, response) = call(
            &state,
            request(Method::POST, &uri, Some("test-token"), Some(body.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "missing write token");
        assert_eq!(response["code"], "forbidden");

        let mut with_bad_token =
            request(Method::POST, &uri, Some("test-token"), Some(body.clone()));
        with_bad_token
            .headers_mut()
            .insert("x-agent-fleet-git-token", "wrong".parse().unwrap());
        let (status, _, _) = call(&state, with_bad_token).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let mut with_token = request(Method::POST, &uri, Some("test-token"), Some(body));
        with_token
            .headers_mut()
            .insert("x-agent-fleet-git-token", "git-secret".parse().unwrap());
        let (status, _, ack) = call(&state, with_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ack["ok"], true);

        let commit_uri = format!("/api/v1/workspaces/{workspace_id}/git/commit");
        let mut commit_request = request(
            Method::POST,
            &commit_uri,
            Some("test-token"),
            Some(json!({"message": "feat: add thing", "paths": ["a.txt"]})),
        );
        commit_request
            .headers_mut()
            .insert("x-agent-fleet-git-token", "git-secret".parse().unwrap());
        let (status, _, commit) = call(&state, commit_request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(commit["commit"], "deadbeef");
        assert_eq!(commit["branch"], "main");
        assert_eq!(commit["summary"], "feat: add thing");

        let checkout_uri = format!("/api/v1/workspaces/{workspace_id}/git/checkout");
        let mut checkout_request = request(
            Method::POST,
            &checkout_uri,
            Some("test-token"),
            Some(json!({"branch": "feature/x", "create": true})),
        );
        checkout_request
            .headers_mut()
            .insert("x-agent-fleet-git-token", "git-secret".parse().unwrap());
        let (status, _, checkout) = call(&state, checkout_request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(checkout["branch"], "feature/x");

        let (status, _, audit) = call(
            &state,
            request(Method::GET, "/api/v1/audit", Some("test-token"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let entries = audit["entries"].as_array().expect("audit entries");
        assert!(entries.iter().any(|entry| {
            entry["action"] == "git.stage"
                && entry["subject"] == workspace_id
                && entry["details"]["outcome"] == "ok"
        }));
        assert!(entries.iter().any(|entry| {
            entry["action"] == "git.stage" && entry["details"]["outcome"] == "denied"
        }));
        assert!(entries.iter().any(|entry| {
            entry["action"] == "git.commit"
                && entry["details"]["outcome"] == "ok"
                && entry["details"]["message"] == "feat: add thing"
        }));
        assert!(entries.iter().any(|entry| {
            entry["action"] == "git.checkout"
                && entry["details"]["outcome"] == "ok"
                && entry["details"]["branch"] == "feature/x"
        }));

        server.abort();
    }

    #[tokio::test]
    async fn git_writes_can_be_disabled() {
        let mut config = test_config();
        config.git.allow_writes = false;
        config.auth.admin_token = "test-token".to_string();
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let state = AppState::new(db, config).await.unwrap();

        let (status, _, response) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/workspaces/ws_1/git/commit",
                Some("test-token"),
                Some(json!({"message": "nope"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(response["code"], "forbidden");

        let entries = state.db.audit.list(None, 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "git.commit");
        assert_eq!(entries[0].subject, "ws_1");
        assert_eq!(entries[0].details["outcome"], "denied");
    }

    async fn create_user_token(
        state: &AppState,
        admin_token: &str,
        org_id: &str,
        name: &str,
        role: &str,
    ) -> (String, String) {
        let (status, _, created) = call(
            state,
            request(
                Method::POST,
                "/api/v1/users",
                Some(admin_token),
                Some(json!({"org_id": org_id, "name": name, "role": role})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create user: {created}");
        (
            created["user"]["id"].as_str().unwrap().to_string(),
            created["token"].as_str().unwrap().to_string(),
        )
    }

    async fn create_org(state: &AppState, admin_token: &str, name: &str) -> String {
        let (status, _, organization) = call(
            state,
            request(
                Method::POST,
                "/api/v1/orgs",
                Some(admin_token),
                Some(json!({"name": name})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create org: {organization}");
        organization["id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn organizations_scope_resources_and_roles_control_writes() {
        let (state, server, _tasks) = start_fake_stack(test_config(), "test-token").await;
        let workspace_id = register_test_workspace(&state, "test-token").await;
        let default_org = fleet_core_protocol::authz::DEFAULT_ORG_ID;
        let other_org = create_org(&state, "test-token", "Other Org").await;

        let (ada_id, ada_token) =
            create_user_token(&state, "test-token", default_org, "Ada", "member").await;
        let (_admin_id, admin_token) =
            create_user_token(&state, "test-token", default_org, "Org Admin", "admin").await;
        let (_viewer_id, viewer_token) =
            create_user_token(&state, "test-token", default_org, "Vera", "viewer").await;
        let (_, other_token) =
            create_user_token(&state, "test-token", &other_org, "Other", "member").await;

        // Machine list and access are scoped to the owning organization.
        let (status, _, machines) = call(
            &state,
            request(Method::GET, "/api/v1/machines", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(machines.as_array().unwrap().len(), 1);

        let (status, _, machines) = call(
            &state,
            request(Method::GET, "/api/v1/machines", Some(&other_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(machines, json!([]));
        let (status, _, _) = call(
            &state,
            request(
                Method::GET,
                "/api/v1/machines/m_test",
                Some(&other_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _, _) = call(
            &state,
            request(
                Method::GET,
                &format!("/api/v1/workspaces/{workspace_id}"),
                Some(&other_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Viewers are read-only.
        let (status, _, body) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/workspaces/{workspace_id}/git/stage"),
                Some(&viewer_token),
                Some(json!({"paths": ["a.txt"]})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "forbidden");
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/audit", Some(&viewer_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Members can write and their name becomes the audit actor.
        let (status, _, ack) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/workspaces/{workspace_id}/git/stage"),
                Some(&ada_token),
                Some(json!({"paths": ["a.txt"]})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ack["ok"], true);
        let entries = state.db.audit.list(None, 20).await.unwrap();
        assert!(entries.iter().any(|entry| {
            entry.action == "git.stage" && entry.actor == "Ada" && entry.details["outcome"] == "ok"
        }));

        // Only admins manage users; admins are scoped to their organization.
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/users", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, users) = call(
            &state,
            request(Method::GET, "/api/v1/users", Some(&admin_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<&str> = users
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|user| user["name"].as_str())
            .collect();
        assert!(names.contains(&"Ada"));
        assert!(names.contains(&"Org Admin"));
        assert!(!names.contains(&"Other"));

        // Audit visibility is organization-scoped.
        let (status, _, audit) = call(
            &state,
            request(Method::GET, "/api/v1/audit", Some(&admin_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(audit["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["action"] == "git.stage"));

        // Token rotation invalidates the old token immediately.
        let (status, _, rotated) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{ada_id}/rotate-token"),
                Some(&admin_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let new_token = rotated["token"].as_str().unwrap().to_string();
        assert_ne!(new_token, ada_token);
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/machines", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/machines", Some(&new_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Admins cannot delete users from another organization.
        let (status, _, _) = call(
            &state,
            request(
                Method::DELETE,
                "/api/v1/users/_missing",
                Some(&new_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "members cannot delete users");

        // The legacy admin can move the machine; the new owner then sees it.
        let (status, _, machine) = call(
            &state,
            request(
                Method::PUT,
                "/api/v1/machines/m_test/org",
                Some("test-token"),
                Some(json!({"org_id": other_org})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "assign org: {machine}");
        let (status, _, machines) = call(
            &state,
            request(Method::GET, "/api/v1/machines", Some(&other_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(machines.as_array().unwrap().len(), 1);
        // The former owner (default org) no longer sees the machine.
        let (status, _, former) = call(
            &state,
            request(Method::GET, "/api/v1/machines", Some(&new_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(former, json!([]));

        server.abort();
    }

    #[tokio::test]
    async fn git_writes_without_a_write_token_are_authorized_by_the_bearer_token() {
        let state = test_state().await;
        let (status, _, response) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/workspaces/ws_missing/git/stage",
                Some("test-token"),
                Some(json!({"paths": ["a.txt"]})),
            ),
        )
        .await;
        // Authorization passed; the workspace lookup is what fails here.
        assert_eq!(status, StatusCode::NOT_FOUND, "response: {response}");
    }
}

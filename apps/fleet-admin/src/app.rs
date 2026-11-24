//! Axum application assembly: API routes, machine WebSocket, CORS and the
//! optional single-page-app static fallback.

use axum::http::{header, HeaderName, HeaderValue, Method};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::routes;
use crate::state::AppState;

/// Build the full fleet-admin router.
///
/// * `/api/v1/*` – REST plus the browser event WebSocket.
/// * `/ws/machine` – machine reverse connection (the gateway authenticates).
/// * everything else – the built web UI when `web.static_dir` exists.
pub fn build_router(state: AppState) -> Router {
    let api = routes::api_router(state.clone()).with_state(state.clone());
    let machine_ws = state.gateway.clone().router();

    let mut router = Router::new().nest("/api/v1", api).nest("/ws", machine_ws);

    let mut spa_fallback = false;
    if let Some(directory) = state.config.web.static_dir.as_deref() {
        if directory.is_dir() {
            let index = directory.join("index.html");
            if index.is_file() {
                // Serve real assets and fall back to index.html so client-side
                // routes survive a page reload. Unmatched `/api/v1` paths are
                // handled by the API router's own JSON fallback.
                router = router
                    .fallback_service(ServeDir::new(directory).fallback(ServeFile::new(index)));
                spa_fallback = true;
            }
        }
    }

    // Layers are applied after the SPA fallback so the metrics and security
    // layers also cover static asset requests. The metrics layer is outermost
    // so it observes the final status of every response.
    let metrics = state.metrics.clone();
    router
        .layer(axum::middleware::from_fn(security_headers))
        .layer(cors_layer())
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let metrics = metrics.clone();
                async move {
                    crate::middleware::metrics::track_metrics(metrics, spa_fallback, request, next)
                        .await
                }
            },
        ))
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
            HeaderName::from_static("x-fleet-git-token"),
            HeaderName::from_static("x-fleet-org"),
            HeaderName::from_static("x-csrf-token"),
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
    use fleet_core_config::{AdminConfig, OAuthProviderConfig};
    use fleet_core_connection::{ClientConfig, DaemonRequestHandler, MachineConnectionClient};
    use fleet_core_protocol::agent::{
        AgentCapabilities, AgentMessage, AgentSession, AgentToolCall, AgentTurn, MessageRole,
        ProviderInfo, ProviderKind, SessionStatus, ToolCallStatus,
    };
    use fleet_core_protocol::authz::UserRole;
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
    use crate::oauth::{OAuthError, OAuthProfile, OAuthProvider, OAuthProviderKind, OAuthRegistry};

    fn test_config() -> AdminConfig {
        AdminConfig::default()
    }

    async fn test_state() -> AppState {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        AppState::new(db, config).await.unwrap()
    }

    async fn test_state_with(config: AdminConfig) -> AppState {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        AppState::new(db, config).await.unwrap()
    }

    async fn test_state_with_oauth(config: AdminConfig, oauth: OAuthRegistry) -> AppState {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        AppState::new_with_oauth(db, config, oauth).await.unwrap()
    }

    /// Register a paired machine with a per-machine token hash.
    async fn pair_test_machine(state: &AppState, machine_id: &str, token: &str, public_key: &str) {
        let now = fleet_core_common::now();
        sqlx::query(
            r#"
            INSERT INTO machines (
                id, name, hostname, os, arch, daemon_version, protocol_version,
                status, public_key, providers_json, workspaces_json,
                created_at, updated_at, token_hash, token_created_at,
                paired_at, paired_by
            ) VALUES (
                ?1, ?1, '', '', '', '', 1, 'offline', ?2, '[]', '[]',
                ?3, ?3, ?4, ?3, ?3, 'test'
            )
            ON CONFLICT(id) DO UPDATE SET
                public_key = excluded.public_key,
                token_hash = excluded.token_hash,
                paired_at = excluded.paired_at,
                paired_by = excluded.paired_by
            "#,
        )
        .bind(machine_id)
        .bind(public_key)
        .bind(now.to_rfc3339())
        .bind(fleet_core_auth::hash_token(token))
        .execute(state.db.pool())
        .await
        .unwrap();
    }

    async fn seed_machine(state: &AppState, id: &str) {
        let timestamp = now();
        state
            .db
            .machines
            .upsert(&fleet_core_protocol::machine::Machine {
                id: id.to_string(),
                name: id.to_string(),
                hostname: String::new(),
                os: String::new(),
                arch: String::new(),
                daemon_version: String::new(),
                protocol_version: fleet_core_protocol::PROTOCOL_VERSION,
                status: fleet_core_protocol::machine::MachineStatus::Offline,
                created_at: timestamp,
                updated_at: timestamp,
                connected_at: None,
                last_seen_at: None,
                providers: Vec::new(),
                workspaces: Vec::new(),
                workspace_count: 0,
                public_key: None,
            })
            .await
            .unwrap();
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
    async fn http_metrics_use_matched_route_templates() {
        let state = test_state().await;
        let router = build_router(state.clone());

        let response = router
            .clone()
            .oneshot(request(Method::GET, "/api/v1/health", None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // A templated route carrying an identifier must be labelled with the
        // route template, never with the raw path.
        let response = router
            .clone()
            .oneshot(request(
                Method::GET,
                "/api/v1/machines/m_secret_id",
                Some("test-token"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Unmatched API paths collapse into a single bounded label.
        let response = router
            .oneshot(request(
                Method::GET,
                "/api/v1/unknown/raw-secret-value",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let (text, _) = state.metrics.render();
        assert!(
            text.contains(
                "fleet_admin_http_requests_total{method=\"GET\",route=\"/api/v1/health\",status=\"200\"} 1"
            ),
            "health response was not recorded:\n{text}"
        );
        assert!(
            text.contains("route=\"/api/v1/machines/{id}\""),
            "route template missing:\n{text}"
        );
        assert!(
            text.contains("route=\"unmatched\""),
            "unmatched label missing:\n{text}"
        );
        assert!(!text.contains("m_secret_id"), "raw path leaked:\n{text}");
        assert!(
            !text.contains("raw-secret-value"),
            "raw path leaked:\n{text}"
        );
    }

    #[tokio::test]
    async fn active_sessions_of_offline_machines_become_interrupted() {
        let state = test_state().await;
        seed_machine(&state, "m_offline").await;
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

    #[tokio::test]
    async fn unrevoke_never_revives_the_old_token() {
        let state = test_state().await;
        pair_test_machine(&state, "m_revive", "flm_live", "key-a").await;

        // Simulate a crash between the revocation commit and the machine-row
        // delete: the row (and formerly its token hash) survives.
        state
            .db
            .machines
            .revoke("m_revive", Some("test"))
            .await
            .unwrap();
        assert!(state
            .db
            .machines
            .by_token_hash(&fleet_core_auth::hash_token("flm_live"))
            .await
            .unwrap()
            .is_none());

        crate::services::machine_service::MachineService::unrevoke(&state, "m_revive", None)
            .await
            .unwrap();
        assert!(
            state
                .db
                .machines
                .by_token_hash(&fleet_core_auth::hash_token("flm_live"))
                .await
                .unwrap()
                .is_none(),
            "unrevocation must not restore the credential"
        );
        assert_eq!(
            state
                .db
                .machines
                .machine_state("m_revive")
                .await
                .unwrap()
                .pinned_key
                .as_deref(),
            Some("key-a")
        );
    }

    fn request(
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Request<Body> {
        request_with_headers(method, uri, token, &[], body)
    }

    fn request_with_headers(
        method: Method,
        uri: &str,
        token: Option<&str>,
        extra: &[(&str, &str)],
        body: Option<Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in extra {
            builder = builder.header(*name, *value);
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

    /// Open the event socket with a bearer header, which the browser cannot do
    /// and which the tests use instead of minting a ticket.
    async fn connect_event_socket(
        address: std::net::SocketAddr,
        token: &str,
    ) -> tokio_tungstenite::tungstenite::Result<(
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    )> {
        connect_event_socket_for_org(address, token, None).await
    }

    async fn connect_event_socket_for_org(
        address: std::net::SocketAddr,
        token: &str,
        org_id: Option<&str>,
    ) -> tokio_tungstenite::tungstenite::Result<(
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    )> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = format!("ws://{address}/api/v1/events").into_client_request()?;
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, format!("Bearer {token}").parse()?);
        if let Some(org_id) = org_id {
            request
                .headers_mut()
                .insert(crate::middleware::auth::ORG_HEADER, org_id.parse()?);
        }
        tokio_tungstenite::connect_async(request).await
    }

    /// `Set-Cookie` values from a response.
    fn set_cookies(headers: &axum::http::HeaderMap) -> Vec<String> {
        headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_owned)
            .collect()
    }

    /// Join the `name=value` parts of `Set-Cookie` values into a request header.
    fn cookie_pairs(cookies: &[String]) -> String {
        cookies
            .iter()
            .filter_map(|cookie| cookie.split(';').next())
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn cookie_value_of(cookies: &[String], name: &str) -> Option<String> {
        let prefix = format!("{name}=");
        cookies.iter().find_map(|cookie| {
            let pair = cookie.split(';').next()?;
            pair.strip_prefix(&prefix).map(str::to_owned)
        })
    }

    fn cookie_request(
        method: Method,
        uri: &str,
        cookie: &str,
        csrf: Option<&str>,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, cookie);
        if let Some(csrf) = csrf {
            builder = builder.header(crate::auth::CSRF_HEADER, csrf);
        }
        builder.body(Body::empty()).unwrap()
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

    async fn call_raw(
        state: &AppState,
        request: Request<Body>,
    ) -> (StatusCode, axum::http::HeaderMap, axum::body::Bytes) {
        let response = build_router(state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, bytes)
    }

    #[tokio::test]
    async fn a_backup_is_bootstrap_only_and_is_a_sqlite_file() {
        // VACUUM INTO from an in-memory database does not produce a file, so
        // this test uses a file-backed admin database like a real deployment.
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("admin.db");
        let db = Arc::new(AdminStorage::connect(&database).await.unwrap());
        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        config.storage.database = database;
        let state = AppState::new(db, config).await.unwrap();
        let (_, member_token) =
            create_user_token(&state, "test-token", "org_default", "Ada", "admin").await;

        let (status, _, _) = call_raw(
            &state,
            request(
                Method::GET,
                "/api/v1/admin/backup",
                Some(&member_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, headers, bytes) = call_raw(
            &state,
            request(
                Method::GET,
                "/api/v1/admin/backup",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(headers[header::CONTENT_TYPE], "application/vnd.sqlite3");
        assert!(bytes.starts_with(b"SQLite format 3\0"));
        assert!(headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("fleet-admin-"));
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
    async fn ready_is_public_and_reports_deep_checks() {
        let state = test_state().await;
        let (status, _, body) =
            call(&state, request(Method::GET, "/api/v1/ready", None, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ready"], true);
        assert_eq!(body["checks"]["database"]["ok"], true);
        assert_eq!(body["checks"]["migrations"]["ok"], true);
        assert_eq!(body["checks"]["web"]["ok"], true);
        assert_eq!(body["checks"]["web"]["detail"], "not configured");
    }

    #[tokio::test]
    async fn ready_fails_when_the_web_bundle_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        config.web.static_dir = Some(dir.path().join("missing"));
        let state = test_state_with(config).await;

        let (status, _, body) =
            call(&state, request(Method::GET, "/api/v1/ready", None, None)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["ready"], false);
        assert_eq!(body["checks"]["database"]["ok"], true);
        assert_eq!(body["checks"]["migrations"]["ok"], true);
        assert_eq!(body["checks"]["web"]["ok"], false);
        assert_eq!(
            body["checks"]["web"]["reason"],
            "static directory is missing"
        );
    }

    #[tokio::test]
    async fn ready_fails_when_the_web_bundle_has_no_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        config.web.static_dir = Some(dir.path().to_path_buf());
        let state = test_state_with(config).await;

        let (status, _, body) =
            call(&state, request(Method::GET, "/api/v1/ready", None, None)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["ready"], false);
        assert_eq!(body["checks"]["web"]["ok"], false);
        assert_eq!(body["checks"]["web"]["reason"], "index.html is missing");
    }

    #[tokio::test]
    async fn ready_fails_when_migrations_are_not_applied() {
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        sqlx::query("DROP TABLE _sqlx_migrations")
            .execute(db.pool())
            .await
            .unwrap();
        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        let state = AppState::new(db, config).await.unwrap();

        let (status, _, body) =
            call(&state, request(Method::GET, "/api/v1/ready", None, None)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["ready"], false);
        assert_eq!(body["checks"]["database"]["ok"], true);
        assert_eq!(body["checks"]["migrations"]["ok"], false);
        let reason = body["checks"]["migrations"]["reason"].as_str().unwrap();
        assert!(reason.contains("migration"), "{reason}");
    }

    #[tokio::test]
    async fn ready_fails_when_the_database_is_unavailable() {
        let state = test_state().await;
        state.db.pool().close().await;

        let (status, _, body) =
            call(&state, request(Method::GET, "/api/v1/ready", None, None)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["ready"], false);
        assert_eq!(body["checks"]["database"]["ok"], false);
        assert_eq!(
            body["checks"]["database"]["reason"],
            "database query failed"
        );
        assert_eq!(body["checks"]["migrations"]["ok"], false);
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
    async fn an_oversized_idempotency_key_is_rejected() {
        let state = test_state().await;
        let oversized = "k".repeat(201);
        let (status, _, body) = call(
            &state,
            request_with_headers(
                Method::POST,
                "/api/v1/sessions/s_1/permissions/p_1/approve",
                Some("test-token"),
                &[("idempotency-key", oversized.as_str())],
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_request");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("Idempotency-Key"));
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

        let (mut socket, _) = connect_event_socket(address, "test-token")
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
                    let parsed: PlatformEvent = serde_json::from_str(&text).unwrap();
                    // Heartbeats share the socket and may arrive first.
                    if parsed.event_type == "agent.event" {
                        received = Some(parsed);
                        break;
                    }
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

    #[tokio::test]
    async fn browser_event_socket_emits_heartbeats() {
        let state = test_state().await;
        let app = build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (mut socket, _) = connect_event_socket(address, "test-token")
            .await
            .expect("event socket should upgrade");

        let mut heartbeat: Option<PlatformEvent> = None;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_secs(5), socket.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                    let event: PlatformEvent = serde_json::from_str(&text).unwrap();
                    if event.event_type == "platform.heartbeat" {
                        heartbeat = Some(event);
                        break;
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(err))) => panic!("websocket error: {err}"),
                Ok(None) => panic!("websocket closed unexpectedly"),
                Err(_) => continue,
            }
        }

        let heartbeat = heartbeat.expect("the socket should emit a heartbeat");
        assert!(
            heartbeat.payload["server_time"].is_string(),
            "heartbeat should carry the server time"
        );
        assert!(heartbeat.machine_id.is_none());

        server.abort();
    }

    /// Minimal daemon used by the loopback end-to-end test.
    struct FakeDaemon {
        sessions: Mutex<Vec<AgentSession>>,
        last_idempotency_key: Mutex<Option<String>>,
    }

    impl FakeDaemon {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(Vec::new()),
                last_idempotency_key: Mutex::new(None),
            }
        }

        async fn last_idempotency_key(&self) -> Option<String> {
            self.last_idempotency_key.lock().await.clone()
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
                methods::SESSIONS_SEND_MESSAGE => {
                    let key = payload
                        .get("options")
                        .and_then(|options| options.get("idempotency_key"))
                        .and_then(|key| key.as_str())
                        .map(str::to_string);
                    *self.last_idempotency_key.lock().await = key;
                    Ok(json!({"ok": true}))
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
                            session_id: session.id,
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
                        questions: vec![],
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

        let response = build_router(state.clone())
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

        let (metrics, _) = state.metrics.render();
        assert!(
            metrics.contains("route=\"spa\""),
            "SPA fallback should be labelled spa:\n{metrics}"
        );
        assert!(
            !metrics.contains("/machines/example"),
            "SPA raw path leaked into metrics:\n{metrics}"
        );
    }

    #[tokio::test]
    async fn machine_websocket_uses_the_machine_token_not_the_admin_token() {
        let mut config = test_config();
        config.auth.admin_token = "admin-token".to_string();
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

        let machine_identity = MachineIdentity::generate();
        pair_test_machine(
            &state,
            "m_machine",
            "machine-token",
            &machine_identity.public_key_base64(),
        )
        .await;
        let machine_client = MachineConnectionClient::new(
            ClientConfig::new(
                format!("ws://{address}/ws/machine"),
                "machine-token",
                "m_machine",
                "Machine Token",
            ),
            machine_identity,
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
        seed_machine(&state, "m_default").await;
        seed_machine(&state, "m_other").await;
        let other_org = team_org(&state, "Other").await;
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
        let user = state
            .db
            .users
            .create(
                Some("ada@example.com"),
                "Ada",
                None,
                Some(&fleet_core_auth::hash_token(&token)),
                Some(fleet_core_common::now()),
                None,
            )
            .await
            .unwrap();
        state
            .db
            .memberships
            .upsert("org_default", &user.id, UserRole::Member, None)
            .await
            .unwrap();

        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (mut socket, _) = connect_event_socket_for_org(address, &token, Some("org_default"))
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
        let rejected = connect_event_socket(address, "bogus").await;
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

        let identity = MachineIdentity::generate();
        pair_test_machine(
            &state,
            "m_test",
            "test-machine-token",
            &identity.public_key_base64(),
        )
        .await;
        let client_config = ClientConfig::new(
            format!("ws://{address}/ws/machine"),
            "test-machine-token",
            "m_test",
            "Test Machine",
        );
        let fake_daemon = Arc::new(FakeDaemon::new());
        let client = MachineConnectionClient::new(client_config, identity, fake_daemon.clone());
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

        // A retry-safe mutation forwards its Idempotency-Key to the daemon.
        let (status, _, body) = call(
            &state,
            request_with_headers(
                Method::POST,
                &format!("/api/v1/sessions/{session_id}/messages"),
                Some("test-token"),
                &[("idempotency-key", "retry-1")],
                Some(json!({"content": "hello"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(
            fake_daemon.last_idempotency_key().await.as_deref(),
            Some("retry-1")
        );

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
    async fn machine_token_rotation_is_confirmed_and_invalidates_the_old_token() {
        let state = test_state().await;
        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let identity = MachineIdentity::generate();
        pair_test_machine(&state, "m_rot", "token-old", &identity.public_key_base64()).await;

        let dir = tempfile::tempdir().unwrap();
        let token_path = dir.path().join("machine.key.token");
        fleet_core_auth::save_machine_token(&token_path, "token-old").unwrap();

        let mut client_config = ClientConfig::new(
            format!("ws://{address}/ws/machine"),
            "token-old",
            "m_rot",
            "Rotated Machine",
        );
        client_config.reconnect_initial = Duration::from_millis(100);
        client_config.reconnect_max = Duration::from_millis(500);
        client_config.token_file = Some(token_path.clone());
        let client =
            MachineConnectionClient::new(client_config, identity, Arc::new(FakeDaemon::new()));
        let shutdown = CancellationToken::new();
        let client_task = tokio::spawn(client.clone().run(shutdown.clone()));
        for _ in 0..100 {
            if state.connections.is_online("m_rot") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(state.connections.is_online("m_rot"));

        let (status, _, body) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/machines/m_rot/rotate-token",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["machine_id"], "m_rot");

        let rotated = std::fs::read_to_string(&token_path)
            .unwrap()
            .trim()
            .to_string();
        assert!(rotated.starts_with("flm_"));
        assert_ne!(rotated, "token-old");
        assert_eq!(
            state
                .db
                .machines
                .by_token_hash(&fleet_core_auth::hash_token("token-old"))
                .await
                .unwrap(),
            None,
            "the old token must stop authenticating once the rotation is confirmed"
        );
        assert_eq!(
            state
                .db
                .machines
                .by_token_hash(&fleet_core_auth::hash_token(&rotated))
                .await
                .unwrap()
                .as_deref(),
            Some("m_rot")
        );

        // The retired credential can no longer open a machine socket.
        {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut upgrade = format!("ws://{address}/ws/machine")
                .into_client_request()
                .unwrap();
            upgrade
                .headers_mut()
                .insert(header::AUTHORIZATION, "Bearer token-old".parse().unwrap());
            match tokio_tungstenite::connect_async(upgrade).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                }
                other => panic!("expected an HTTP 401 for the old token, got {other:?}"),
            }
        }

        // The live daemon reconnects with the token it just persisted.
        state.connections.disconnect("m_rot");
        let mut reconnected = false;
        for _ in 0..100 {
            if state.connections.is_online("m_rot") {
                reconnected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            reconnected,
            "daemon did not reconnect with the rotated token"
        );

        shutdown.cancel();
        client_task.abort();
        server.abort();
    }

    #[tokio::test]
    async fn an_unconfirmed_rotation_blocks_a_second_token() {
        let state = test_state().await;
        let identity = MachineIdentity::generate();
        pair_test_machine(&state, "m_off", "token-old", &identity.public_key_base64()).await;

        // The machine is offline, so the daemon cannot confirm the rotation.
        let (status, _, body) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/machines/m_off/rotate-token",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(state
            .db
            .machines
            .pending_token_hash("m_off")
            .await
            .unwrap()
            .is_some());

        // Losing the in-memory plaintext (for example, an admin restart) must
        // not let a second, different token be offered while the first may
        // still be on the daemon's disk.
        state.token_rotations.remove("m_off");
        let (status, _, body) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/machines/m_off/rotate-token",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], error_codes::MACHINE_TOKEN_ROTATION_PENDING);

        // Canceling clears the pending hash and leaves the current token valid.
        let (status, _, _) = call(
            &state,
            request(
                Method::DELETE,
                "/api/v1/machines/m_off/rotate-token",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            state.db.machines.pending_token_hash("m_off").await.unwrap(),
            None
        );
        assert!(state
            .db
            .machines
            .by_token_hash(&fleet_core_auth::hash_token("token-old"))
            .await
            .unwrap()
            .is_some());
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
        seed_machine(&state, "m_offline").await;
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
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        let state = AppState::new(db, config).await.unwrap();
        let identity = MachineIdentity::generate();
        pair_test_machine(
            &state,
            "m_test",
            "test-machine-token",
            &identity.public_key_base64(),
        )
        .await;
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
                "test-machine-token",
                "m_test",
                "Test Machine",
            ),
            identity,
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
            .insert("x-fleet-git-token", "wrong".parse().unwrap());
        let (status, _, _) = call(&state, with_bad_token).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let mut with_token = request(Method::POST, &uri, Some("test-token"), Some(body));
        with_token
            .headers_mut()
            .insert("x-fleet-git-token", "git-secret".parse().unwrap());
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
            .insert("x-fleet-git-token", "git-secret".parse().unwrap());
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
            .insert("x-fleet-git-token", "git-secret".parse().unwrap());
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
        let email = format!("{}@example.com", name.to_lowercase().replace(' ', "."));
        let (status, _, created) = call(
            state,
            request(
                Method::POST,
                "/api/v1/users",
                Some(admin_token),
                Some(json!({
                    "org_id": org_id,
                    "email": email,
                    "name": name,
                    "role": role,
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create user: {created}");
        (
            created["user"]["id"].as_str().unwrap().to_string(),
            created["token"].as_str().unwrap().to_string(),
        )
    }

    /// Create a team organization directly in storage.
    async fn team_org(state: &AppState, name: &str) -> fleet_core_protocol::authz::Organization {
        state
            .db
            .organizations
            .create(
                fleet_core_protocol::authz::OrganizationKind::Team,
                name,
                None,
            )
            .await
            .unwrap()
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
        organization["organization"]["id"]
            .as_str()
            .unwrap()
            .to_string()
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
    async fn renaming_a_foreign_organization_is_not_found() {
        let state = test_state().await;
        let other_org = create_org(&state, "test-token", "Other Org").await;
        let (_ada_id, ada_token) =
            create_user_token(&state, "test-token", "org_default", "Ada", "member").await;

        // A non-member must not learn whether the organization exists: the
        // membership check runs before the owner check.
        let (status, _, body) = call(
            &state,
            request(
                Method::PATCH,
                &format!("/api/v1/orgs/{other_org}"),
                Some(&ada_token),
                Some(json!({"name": "Hijacked"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "response: {body}");
        assert_eq!(body["code"], "not_found");

        // A member of the organization still gets a role error, not a rename.
        let (status, _, body) = call(
            &state,
            request(
                Method::PATCH,
                "/api/v1/orgs/org_default",
                Some(&ada_token),
                Some(json!({"name": "Renamed"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "response: {body}");

        let (status, _, renamed) = call(
            &state,
            request(
                Method::PATCH,
                &format!("/api/v1/orgs/{other_org}"),
                Some("test-token"),
                Some(json!({"name": "Renamed"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "rename: {renamed}");
        assert_eq!(renamed["organization"]["name"], "Renamed");
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

    #[tokio::test]
    async fn user_tokens_bootstrap_a_browser_session_with_csrf() {
        let state = test_state().await;
        let (_ada_id, ada_token) =
            create_user_token(&state, "test-token", "org_default", "Ada", "member").await;

        // The legacy bootstrap token cannot get a browser session.
        let (status, _, _) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/auth/session",
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // A foreign origin is rejected even with a valid user token.
        let mut evil_origin = request(Method::POST, "/api/v1/auth/session", Some(&ada_token), None);
        evil_origin
            .headers_mut()
            .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        let (status, _, _) = call(&state, evil_origin).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, headers, me) = call(
            &state,
            request(Method::POST, "/api/v1/auth/session", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "session: {me}");
        assert_eq!(me["kind"], "user");
        let cookies = set_cookies(&headers);
        let csrf = cookie_value_of(&cookies, "fleet_csrf").expect("csrf cookie");
        let jar = cookie_pairs(&cookies);

        // The session cookie alone authenticates.
        let (status, _, me) = call(
            &state,
            cookie_request(Method::GET, "/api/v1/me", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["user"]["name"], "Ada");

        // Cookie-authenticated unsafe methods need the double-submit token.
        let (status, _, body) = call(
            &state,
            cookie_request(Method::POST, "/api/v1/realtime/ticket", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "forbidden");
        let (status, _, ticket) = call(
            &state,
            cookie_request(Method::POST, "/api/v1/realtime/ticket", &jar, Some(&csrf)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "ticket: {ticket}");
        assert!(ticket["ticket"].as_str().is_some());

        // Logout requires CSRF, revokes the session and clears both cookies.
        let (status, _, _) = call(
            &state,
            cookie_request(Method::POST, "/api/v1/auth/logout", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, headers, _) = call(
            &state,
            cookie_request(Method::POST, "/api/v1/auth/logout", &jar, Some(&csrf)),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let cleared = set_cookies(&headers);
        assert!(cleared
            .iter()
            .any(|cookie| cookie.starts_with("fleet_session=;")));
        let (status, _, _) = call(
            &state,
            cookie_request(Method::GET, "/api/v1/me", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Viewers may upgrade their token even though their role is read-only.
        let (_viewer_id, viewer_token) =
            create_user_token(&state, "test-token", "org_default", "Vera", "viewer").await;
        let (status, _, _) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/auth/session",
                Some(&viewer_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // An idle session is revoked on its next request even before the
        // absolute expiry.
        let (status, headers, _) = call(
            &state,
            request(Method::POST, "/api/v1/auth/session", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let cookies = set_cookies(&headers);
        let session = cookie_value_of(&cookies, "fleet_session").expect("session cookie");
        let jar = cookie_pairs(&cookies);
        sqlx::query("UPDATE auth_sessions SET last_seen_at = ?2 WHERE id_hash = ?1")
            .bind(fleet_core_auth::hash_token(&session))
            .bind((fleet_core_common::now() - chrono::Duration::days(30)).to_rfc3339())
            .execute(state.db.pool())
            .await
            .unwrap();
        let (status, _, _) = call(
            &state,
            cookie_request(Method::GET, "/api/v1/me", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // A session outlives the bearer token that created it, while the token
        // path still enforces its own expiry.
        let (status, headers, _) = call(
            &state,
            request(Method::POST, "/api/v1/auth/session", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let jar = cookie_pairs(&set_cookies(&headers));
        sqlx::query("UPDATE users SET token_expires_at = ?2 WHERE token_hash = ?1")
            .bind(fleet_core_auth::hash_token(&ada_token))
            .bind((fleet_core_common::now() - chrono::Duration::days(1)).to_rfc3339())
            .execute(state.db.pool())
            .await
            .unwrap();
        let (status, _, _) = call(
            &state,
            cookie_request(Method::GET, "/api/v1/me", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, body) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], error_codes::TOKEN_EXPIRED);
    }

    /// OAuth provider used by the callback tests; it never makes a request.
    struct FakeOAuthProvider {
        kind: OAuthProviderKind,
        profile: OAuthProfile,
    }

    #[async_trait]
    impl OAuthProvider for FakeOAuthProvider {
        fn kind(&self) -> OAuthProviderKind {
            self.kind
        }

        fn authorize_url(
            &self,
            redirect_uri: &str,
            state: &str,
            challenge: &str,
            _nonce: Option<&str>,
        ) -> String {
            format!(
                "https://provider.example/authorize?redirect_uri={redirect_uri}&state={state}\
                 &code_challenge={challenge}"
            )
        }

        async fn exchange(
            &self,
            code: &str,
            _redirect_uri: &str,
            _verifier: &str,
            _nonce: Option<&str>,
        ) -> Result<OAuthProfile, OAuthError> {
            if code == "good-code" {
                Ok(self.profile.clone())
            } else {
                Err(OAuthError::Exchange("bad code".into()))
            }
        }
    }

    fn oauth_registry(profile: OAuthProfile) -> OAuthRegistry {
        OAuthRegistry::default().with_provider(Arc::new(FakeOAuthProvider {
            kind: OAuthProviderKind::Github,
            profile,
        }))
    }

    fn oauth_config(bootstrap_emails: &[&str]) -> AdminConfig {
        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        config.auth.github = Some(OAuthProviderConfig {
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
        });
        config.auth.bootstrap_admin_emails = bootstrap_emails
            .iter()
            .map(|email| email.to_string())
            .collect();
        config
    }

    fn fake_profile(email: &str, email_verified: bool) -> OAuthProfile {
        OAuthProfile {
            subject: format!("gh-{email}"),
            email: Some(email.to_string()),
            email_verified,
            name: "Ada".to_string(),
            login: Some("ada".to_string()),
            avatar_url: None,
        }
    }

    async fn oauth_state_param(state: &AppState, provider: &str) -> String {
        let (status, headers, body) = call(
            state,
            request(
                Method::GET,
                &format!("/api/v1/auth/login/{provider}"),
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "login: {body}");
        let location = headers
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("authorize redirect");
        location
            .split("state=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .expect("state parameter")
            .to_string()
    }

    async fn oauth_callback(
        state: &AppState,
        provider: &str,
        code: &str,
        state_param: &str,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        call(
            state,
            request(
                Method::GET,
                &format!("/api/v1/auth/callback/{provider}?code={code}&state={state_param}"),
                None,
                None,
            ),
        )
        .await
    }

    fn callback_location(headers: &axum::http::HeaderMap) -> String {
        headers
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn oauth_login_creates_the_first_user_and_a_browser_session() {
        let state = test_state_with_oauth(
            oauth_config(&["ada@example.com"]),
            oauth_registry(fake_profile("ada@example.com", true)),
        )
        .await;

        let (status, _, options) = call(
            &state,
            request(Method::GET, "/api/v1/auth/providers", None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(options["providers"], json!(["github"]));
        assert_eq!(options["bootstrap_required"], true);
        assert_eq!(options["bootstrap_email_configured"], true);

        let state_param = oauth_state_param(&state, "github").await;
        let (status, headers, _) =
            oauth_callback(&state, "github", "good-code", &state_param).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(callback_location(&headers), "/");
        let cookies = set_cookies(&headers);
        assert!(cookie_value_of(&cookies, "fleet_session").is_some());
        let jar = cookie_pairs(&cookies);

        let (status, _, me) = call(
            &state,
            cookie_request(Method::GET, "/api/v1/me", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["user"]["email"], "ada@example.com");
        assert_eq!(me["role"], "owner");
        let identity = state
            .db
            .auth_identities
            .find("github", "gh-ada@example.com")
            .await
            .unwrap()
            .expect("linked identity");
        assert_eq!(identity.email, "ada@example.com");

        // Replaying a consumed state changes nothing.
        let (status, headers, _) =
            oauth_callback(&state, "github", "good-code", &state_param).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(callback_location(&headers), "/?auth_error=state_invalid");

        let audit = state.db.audit.list(None, 10).await.unwrap();
        assert!(audit.iter().any(|entry| entry.action == "auth.login"));
    }

    #[tokio::test]
    async fn oauth_rejects_ineligible_accounts() {
        // Unverified email.
        let state = test_state_with_oauth(
            oauth_config(&["ada@example.com"]),
            oauth_registry(fake_profile("ada@example.com", false)),
        )
        .await;
        let state_param = oauth_state_param(&state, "github").await;
        let (_, headers, _) = oauth_callback(&state, "github", "good-code", &state_param).await;
        assert_eq!(callback_location(&headers), "/?auth_error=email_unverified");

        // No users yet and the email is not a bootstrap administrator.
        let state = test_state_with_oauth(
            oauth_config(&[]),
            oauth_registry(fake_profile("stranger@example.com", true)),
        )
        .await;
        let state_param = oauth_state_param(&state, "github").await;
        let (_, headers, _) = oauth_callback(&state, "github", "good-code", &state_param).await;
        assert_eq!(callback_location(&headers), "/?auth_error=not_initialized");

        // Once a user exists, a verified stranger gets a personal account.
        let state = test_state_with_oauth(
            oauth_config(&[]),
            oauth_registry(fake_profile("stranger@example.com", true)),
        )
        .await;
        let _ = create_user_token(&state, "test-token", "org_default", "Root", "admin").await;
        let state_param = oauth_state_param(&state, "github").await;
        let (status, headers, _) =
            oauth_callback(&state, "github", "good-code", &state_param).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(callback_location(&headers), "/");
        let created = state
            .db
            .users
            .by_email("stranger@example.com")
            .await
            .unwrap()
            .expect("self-service account");
        assert!(state
            .db
            .organizations
            .personal_for_user(&created.id)
            .await
            .unwrap()
            .is_some());

        // A disabled account cannot sign in even with a verified email.
        let state = test_state_with_oauth(
            oauth_config(&["ada@example.com"]),
            oauth_registry(fake_profile("ada@example.com", true)),
        )
        .await;
        let disabled = state
            .db
            .users
            .create_from_identity("ada@example.com", "Ada", None)
            .await
            .unwrap();
        assert!(state.db.users.disable(&disabled.id).await.unwrap());
        let state_param = oauth_state_param(&state, "github").await;
        let (_, headers, _) = oauth_callback(&state, "github", "good-code", &state_param).await;
        assert_eq!(callback_location(&headers), "/?auth_error=account_disabled");
    }

    #[tokio::test]
    async fn disabling_a_user_revokes_sessions_and_blocks_the_token() {
        let state = test_state().await;
        let (ada_id, ada_token) =
            create_user_token(&state, "test-token", "org_default", "Ada", "member").await;
        let (root_id, root_token) =
            create_user_token(&state, "test-token", "org_default", "Root", "admin").await;

        // Ada signs in through the browser before the account is disabled.
        let (status, headers, _) = call(
            &state,
            request(Method::POST, "/api/v1/auth/session", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let jar = cookie_pairs(&set_cookies(&headers));

        let (status, _, disabled) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{ada_id}/disable"),
                Some(&root_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "disable: {disabled}");
        assert!(disabled["disabled_at"].as_str().is_some());

        // The bearer token is blocked and the browser session is revoked.
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = call(
            &state,
            cookie_request(Method::GET, "/api/v1/me", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Listing users still shows the disabled account so it can be re-enabled.
        let (status, _, users) = call(
            &state,
            request(Method::GET, "/api/v1/users", Some(&root_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(users
            .as_array()
            .unwrap()
            .iter()
            .any(|user| user["name"] == "Ada" && user["disabled_at"].is_string()));

        // Disabling twice is idempotent; enabling restores the bearer token but
        // not the revoked session.
        let (status, _, _) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{ada_id}/disable"),
                Some(&root_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, enabled) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{ada_id}/enable"),
                Some(&root_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "enable: {enabled}");
        assert!(enabled["disabled_at"].is_null());
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&ada_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = call(
            &state,
            cookie_request(Method::GET, "/api/v1/me", &jar, None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Members cannot disable accounts, and administrators cannot disable
        // themselves.
        let (status, _, _) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{root_id}/disable"),
                Some(&ada_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{root_id}/disable"),
                Some(&root_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let audit = state.db.audit.list(None, 30).await.unwrap();
        assert!(audit.iter().any(|entry| entry.action == "user.disable"
            && entry.details["sessions_revoked"].as_u64() == Some(1)));
        assert!(audit.iter().any(|entry| entry.action == "user.enable"));
    }

    #[tokio::test]
    async fn pairing_codes_require_admin_and_are_org_scoped() {
        let state = test_state().await;

        let (status, _, body) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/pairing/codes",
                Some("test-token"),
                Some(json!({"org_id": "org_default"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["org_id"], "org_default");
        assert!(body["code"].as_str().unwrap().contains('-'));

        let member_token = fleet_core_auth::generate_user_token();
        let member = state
            .db
            .users
            .create(
                Some("member@example.com"),
                "Member",
                None,
                Some(&fleet_core_auth::hash_token(&member_token)),
                Some(fleet_core_common::now()),
                None,
            )
            .await
            .unwrap();
        state
            .db
            .memberships
            .upsert("org_default", &member.id, UserRole::Member, None)
            .await
            .unwrap();
        let (status, _, _) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/pairing/codes",
                Some(&member_token),
                Some(json!({})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let other_org = team_org(&state, "Other").await;
        let other_admin = fleet_core_auth::generate_user_token();
        let other_admin_user = state
            .db
            .users
            .create(
                Some("other@example.com"),
                "Other Admin",
                None,
                Some(&fleet_core_auth::hash_token(&other_admin)),
                Some(fleet_core_common::now()),
                None,
            )
            .await
            .unwrap();
        state
            .db
            .memberships
            .upsert(&other_org.id, &other_admin_user.id, UserRole::Admin, None)
            .await
            .unwrap();
        let (status, _, _) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/pairing/codes",
                Some(&other_admin),
                Some(json!({"org_id": "org_default"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let audit = state.db.audit.list(Some("org_default"), 10).await.unwrap();
        assert!(audit
            .iter()
            .any(|entry| entry.action == "pairing.code.create"));
    }

    #[tokio::test]
    async fn user_token_lifecycle_reports_me_ttl_rotation_and_expiry() {
        use chrono::{Duration, Utc};

        let mut config = test_config();
        config.auth.admin_token = "test-token".to_string();
        config.auth.user_token_ttl_days = 30;
        let state = test_state_with(config).await;

        // The bootstrap admin has no user record and no token lifetime.
        let (status, _, me) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some("test-token"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["kind"], "legacy_admin");
        assert_eq!(me["role"], "owner");
        assert!(me["user"].is_null());
        assert!(me["org_id"].is_null());

        // New tokens use the configured default TTL.
        let (status, _, created) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/users",
                Some("test-token"),
                Some(json!({
                    "org_id": "org_default",
                    "email": "ada@example.com",
                    "name": "Ada",
                    "role": "member"
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create user: {created}");
        let user_id = created["user"]["id"].as_str().unwrap().to_string();
        let user_token = created["token"].as_str().unwrap().to_string();
        let issued: chrono::DateTime<Utc> = created["user"]["token_issued_at"]
            .as_str()
            .expect("token_issued_at")
            .parse()
            .unwrap();
        let expires: chrono::DateTime<Utc> = created["user"]["token_expires_at"]
            .as_str()
            .expect("token_expires_at")
            .parse()
            .unwrap();
        assert_eq!(expires - issued, Duration::days(30));

        // A user principal sees its record, organization and role.
        let (status, _, me) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&user_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["kind"], "user");
        assert_eq!(me["user"]["id"], user_id.as_str());
        assert_eq!(me["org_id"], "org_default");
        assert_eq!(me["role"], "member");
        assert!(me["user"]["token_expires_at"].as_str().is_some());

        // An explicit per-token expiry overrides the default.
        let (status, _, weekly) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/users",
                Some("test-token"),
                Some(json!({
                    "org_id": "org_default",
                    "email": "weekly@example.com",
                    "name": "Weekly",
                    "role": "member",
                    "token_expires_in_days": 7
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create weekly: {weekly}");
        let weekly_issued: chrono::DateTime<Utc> = weekly["user"]["token_issued_at"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let weekly_expires: chrono::DateTime<Utc> = weekly["user"]["token_expires_at"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(weekly_expires - weekly_issued, Duration::days(7));

        // Zero means the token never expires.
        let (status, _, never) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/users",
                Some("test-token"),
                Some(json!({
                    "org_id": "org_default",
                    "email": "never@example.com",
                    "name": "Never",
                    "role": "member",
                    "token_expires_in_days": 0
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create never: {never}");
        assert!(never["user"]["token_expires_at"].is_null());

        // Overrides above the hard limit are rejected.
        let (status, _, error) = call(
            &state,
            request(
                Method::POST,
                "/api/v1/users",
                Some("test-token"),
                Some(json!({
                    "org_id": "org_default",
                    "email": "toolong@example.com",
                    "name": "Too Long",
                    "role": "member",
                    "token_expires_in_days": 3651
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], error_codes::INVALID_REQUEST);

        // Rotation without a query parameter keeps the empty-body contract and
        // uses the configured default.
        let (status, _, rotated) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{user_id}/rotate-token"),
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "rotate: {rotated}");
        let rotated_token = rotated["token"].as_str().unwrap().to_string();
        let rotated_expires: chrono::DateTime<Utc> = rotated["token_expires_at"]
            .as_str()
            .expect("rotated expiry")
            .parse()
            .unwrap();
        let now = Utc::now();
        assert!(rotated_expires - now > Duration::days(29));
        assert!(rotated_expires - now < Duration::days(31));
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&user_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&rotated_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // An explicit rotation TTL is honoured, and an oversized one rejected.
        let (status, _, rotated) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{user_id}/rotate-token?expires_in_days=90"),
                Some("test-token"),
                Some(json!({})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let expires: chrono::DateTime<Utc> = rotated["token_expires_at"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(expires - Utc::now() > Duration::days(89));
        assert!(expires - Utc::now() < Duration::days(91));

        let (status, _, error) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{user_id}/rotate-token?expires_in_days=3651"),
                Some("test-token"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], error_codes::INVALID_REQUEST);

        // Viewers may rotate their own token despite the read-only role.
        let (viewer_id, viewer_token) =
            create_user_token(&state, "test-token", "org_default", "Vera", "viewer").await;
        let (status, _, viewer_rotated) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{viewer_id}/rotate-token"),
                Some(&viewer_token),
                None,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "viewer self-rotate: {viewer_rotated}"
        );
        let new_viewer_token = viewer_rotated["token"].as_str().unwrap().to_string();
        let (status, _, _) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&viewer_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, me) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&new_viewer_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["role"], "viewer");

        // A viewer still cannot rotate somebody else's token.
        let (status, _, error) = call(
            &state,
            request(
                Method::POST,
                &format!("/api/v1/users/{user_id}/rotate-token"),
                Some(&new_viewer_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(error["code"], error_codes::FORBIDDEN);

        // Expired tokens fail closed on both REST and the event WebSocket.
        let expired_token = fleet_core_auth::generate_user_token();
        let now = Utc::now();
        let expired = state
            .db
            .users
            .create(
                Some("expired@example.com"),
                "Expired",
                None,
                Some(&fleet_core_auth::hash_token(&expired_token)),
                Some(now - Duration::days(31)),
                Some(now - Duration::days(1)),
            )
            .await
            .unwrap();
        state
            .db
            .memberships
            .upsert("org_default", &expired.id, UserRole::Member, None)
            .await
            .unwrap();
        let (status, _, error) = call(
            &state,
            request(Method::GET, "/api/v1/me", Some(&expired_token), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error["code"], error_codes::TOKEN_EXPIRED);

        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let rejected = connect_event_socket(address, &expired_token).await;
        assert!(
            rejected.is_err(),
            "expired user tokens must not upgrade the event socket"
        );
        server.abort();

        // Audit entries include the expiry but never the plaintext token.
        let entries = state.db.audit.list(None, 50).await.unwrap();
        let create_entry = entries
            .iter()
            .find(|entry| entry.action == "user.create")
            .expect("user.create audit entry");
        assert!(create_entry.details["token_expires_at"].as_str().is_some());
        assert!(create_entry.details.get("token").is_none());
        let rotate_entry = entries
            .iter()
            .find(|entry| entry.action == "user.rotate_token")
            .expect("user.rotate_token audit entry");
        assert!(rotate_entry.details["token_expires_at"].as_str().is_some());
        assert!(rotate_entry.details.get("token").is_none());
    }
}

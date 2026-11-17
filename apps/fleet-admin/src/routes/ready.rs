//! Unauthenticated deep readiness endpoint.
//!
//! `/health` is a cheap liveness probe. This endpoint additionally verifies
//! the dependencies the server needs before it can serve traffic.

use std::path::Path;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Map, Value};

use crate::state::AppState;

/// Report whether the admin has a usable database, schema and web bundle.
///
/// Returns `200` only when every check passes; otherwise `503` with the same
/// envelope so a failing dependency is visible without parsing logs.
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let mut checks = Map::new();

    let database = match state.db.ping().await {
        Ok(()) => json!({"ok": true}),
        Err(error) => {
            tracing::warn!(error = %error, "readiness: database query failed");
            json!({"ok": false, "reason": "database query failed"})
        }
    };
    checks.insert("database".to_string(), database);

    let migrations = match state.db.pending_migrations().await {
        Ok(pending) if pending.is_empty() => json!({"ok": true}),
        Ok(pending) => {
            tracing::warn!(
                count = pending.len(),
                "readiness: migrations are not applied"
            );
            json!({
                "ok": false,
                "reason": format!("{} expected migration(s) not applied", pending.len()),
            })
        }
        Err(error) => {
            tracing::warn!(error = %error, "readiness: migration check failed");
            json!({"ok": false, "reason": "migration state is unreadable"})
        }
    };
    checks.insert("migrations".to_string(), migrations);

    checks.insert(
        "web".to_string(),
        web_check(state.config.web.static_dir.as_deref()),
    );

    let ready = checks
        .values()
        .all(|check| check["ok"].as_bool() == Some(true));
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(json!({"ready": ready, "checks": checks})))
}

/// Check the optional single-page-app bundle.
///
/// The configured path is not echoed back: readiness responses can be exposed
/// to less trusted probes and the operator already knows the path.
fn web_check(static_dir: Option<&Path>) -> Value {
    let Some(directory) = static_dir else {
        return json!({"ok": true, "detail": "not configured"});
    };
    if !directory.is_dir() {
        return json!({"ok": false, "reason": "static directory is missing"});
    }
    if !directory.join("index.html").is_file() {
        return json!({"ok": false, "reason": "index.html is missing"});
    }
    json!({"ok": true})
}

//! Unauthenticated liveness endpoint.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

/// Return server version and the number of live machine connections.
pub async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "machines_online": state.connections.online().len(),
    }))
}

#![allow(clippy::result_large_err)]

//! Business logic that sits between the HTTP routes and the storage/gateway
//! crates.

pub mod acl;
pub mod agent_service;
pub mod event_service;
pub mod git_service;
pub mod machine_service;
pub mod org_service;
pub mod pairing_service;
pub mod policy;
pub mod workspace_service;

use serde::de::DeserializeOwned;

use crate::error::AppError;
use crate::state::AppState;

/// Reject a request that needs a machine which is not connected.
pub(crate) fn ensure_online(state: &AppState, machine_id: &str) -> Result<(), AppError> {
    if state.connections.is_online(machine_id) {
        Ok(())
    } else {
        Err(AppError::machine_offline(machine_id))
    }
}

/// Forward one JSON-RPC method to a machine with the configured timeout.
pub(crate) async fn forward(
    state: &AppState,
    machine_id: &str,
    method: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    ensure_online(state, machine_id)?;
    state
        .connections
        .request_with_timeout(machine_id, method, payload, state.request_timeout())
        .await
        .map_err(AppError::from_api)
}

/// Deserialize a daemon response, reporting an unusable body as `502`.
pub(crate) fn parse_response<T: DeserializeOwned>(
    value: serde_json::Value,
    what: &str,
) -> Result<T, AppError> {
    serde_json::from_value(value)
        .map_err(|err| AppError::bad_gateway(format!("invalid {what} response from daemon: {err}")))
}

/// Serialize an outgoing RPC payload.
pub(crate) fn to_payload<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, AppError> {
    serde_json::to_value(value).map_err(AppError::internal)
}

/// Attach retry semantics from an HTTP `Idempotency-Key` header.
///
/// The daemon dedupes on the key, so a retried mutation executes once.
pub(crate) fn with_idempotency(payload: &mut serde_json::Value, key: Option<&str>) {
    if let Some(key) = key {
        fleet_core_protocol::connection::RequestOptions {
            timeout_ms: None,
            idempotency_key: Some(key.to_string()),
        }
        .embed(payload);
    }
}

/// Convenience conversion used when a storage lookup returns `None`.
pub(crate) fn not_found(what: &str, id: &str) -> AppError {
    AppError::not_found(format!("{what} {id} not found"))
}

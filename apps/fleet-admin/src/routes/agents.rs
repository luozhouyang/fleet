//! Agent and driver catalog routes.
//!
//! The catalog is served per machine because definitions, availability and
//! drivers are machine-local. Clients pass `machine_id` as a query parameter;
//! the daemon-side wire shape is forwarded unchanged so the admin never needs
//! to understand ACP schema types.

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::services::agent_service::AgentService;
use crate::services::not_found;
use crate::state::AppState;

/// Machine selector shared by every catalog route.
#[derive(Debug, Deserialize)]
pub struct MachineQuery {
    pub machine_id: String,
}

/// `GET /api/v1/agents?machine_id=...`
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<MachineQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<Value>>, AppError> {
    acl::ensure_machine_access(&state, &principal, &query.machine_id).await?;
    Ok(Json(
        AgentService::list_agents(&state, &query.machine_id).await?,
    ))
}

/// `GET /api/v1/agents/{id}?machine_id=...`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MachineQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, AppError> {
    acl::ensure_machine_access(&state, &principal, &query.machine_id).await?;
    let agents = AgentService::list_agents(&state, &query.machine_id).await?;
    agents
        .into_iter()
        .find(|agent| agent.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .map(Json)
        .ok_or_else(|| not_found("agent", &id))
}

/// `POST /api/v1/agents/{id}/probe?machine_id=...`
pub async fn probe(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MachineQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, AppError> {
    acl::ensure_machine_access(&state, &principal, &query.machine_id).await?;
    Ok(Json(
        AgentService::probe_agent(&state, &query.machine_id, &id).await?,
    ))
}

/// `GET /api/v1/drivers?machine_id=...`
pub async fn list_drivers(
    State(state): State<AppState>,
    Query(query): Query<MachineQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<Value>>, AppError> {
    acl::ensure_machine_access(&state, &principal, &query.machine_id).await?;
    Ok(Json(
        AgentService::list_drivers(&state, &query.machine_id).await?,
    ))
}

/// `GET /api/v1/drivers/{id}?machine_id=...`
pub async fn get_driver(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MachineQuery>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, AppError> {
    acl::ensure_machine_access(&state, &principal, &query.machine_id).await?;
    let drivers = AgentService::list_drivers(&state, &query.machine_id).await?;
    drivers
        .into_iter()
        .find(|driver| driver.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .map(Json)
        .ok_or_else(|| not_found("driver", &id))
}

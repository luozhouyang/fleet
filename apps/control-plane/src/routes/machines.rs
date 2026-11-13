//! Machine endpoints.

use agent_fleet_protocol::agent::ProviderInfo;
use agent_fleet_protocol::machine::Machine;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::error::AppError;
use crate::services::machine_service::MachineService;
use crate::state::AppState;

/// `GET /api/v1/machines`
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Machine>>, AppError> {
    Ok(Json(MachineService::list(&state).await?))
}

/// `GET /api/v1/machines/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Machine>, AppError> {
    Ok(Json(MachineService::get(&state, &id).await?))
}

/// `DELETE /api/v1/machines/{id}`
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    MachineService::remove(&state, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/machines/{id}/providers`
pub async fn providers(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<ProviderInfo>>, AppError> {
    Ok(Json(MachineService::providers(&state, &id).await?))
}

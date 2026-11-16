//! Pairing-code endpoint.

use axum::extract::State;
use axum::Extension;
use axum::Json;
use fleet_core_protocol::pairing::{CreatePairingCodeRequest, PairingCodeResponse};

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::Principal;
use crate::services::{acl, pairing_service::PairingService};
use crate::state::AppState;

/// `POST /api/v1/pairing/codes`
pub async fn create_code(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreatePairingCodeRequest>,
) -> Result<Json<PairingCodeResponse>, AppError> {
    acl::ensure_admin(&principal)?;
    Ok(Json(
        PairingService::create_code(&state, &principal, request).await?,
    ))
}

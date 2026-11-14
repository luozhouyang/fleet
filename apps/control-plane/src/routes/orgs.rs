//! Organization administration endpoints.

use agent_fleet_protocol::authz::{CreateOrganizationRequest, Organization};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::state::AppState;

/// `GET /api/v1/orgs` – administrators only.
pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<Organization>>, AppError> {
    acl::ensure_admin(&principal)?;
    let mut organizations = state.db.list_organizations().await?;
    if let Some(org_id) = principal.org_id() {
        organizations.retain(|organization| organization.id == org_id);
    }
    Ok(Json(organizations))
}

/// `POST /api/v1/orgs` – only the legacy super-admin token can create
/// organizations, because a user admin is scoped to their current org.
pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateOrganizationRequest>,
) -> Result<Json<Organization>, AppError> {
    if !principal.is_legacy_admin() {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            agent_fleet_common::error_codes::FORBIDDEN,
            "only the control-plane admin token can create organizations",
        ));
    }
    let name = request.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("organization name must not be empty"));
    }
    if name.len() > 120 {
        return Err(AppError::bad_request(
            "organization name is limited to 120 characters",
        ));
    }
    let organization = state.db.create_organization(name).await?;
    if let Err(error) = state
        .db
        .append_audit_log(
            &principal.actor,
            Some(&organization.id),
            "org.create",
            &organization.id,
            serde_json::json!({"name": organization.name}),
        )
        .await
    {
        tracing::warn!(error = %error, "failed to audit organization creation");
    }
    Ok(Json(organization))
}

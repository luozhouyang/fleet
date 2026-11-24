//! Organization, membership and invitation endpoints.
//!
//! The active organization is selected per request through `X-Fleet-Org`. These
//! routes operate on one organization at a time and never trust a path segment
//! for authorization: every handler asks the policy layer first.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use fleet_core_protocol::authz::{
    AddMemberRequest, CreateInvitationRequest, CreateOrganizationRequest, CreatedOrgInvitation,
    Membership, OrgInvitation, OrgMember, OrganizationSummary, UpdateMemberRequest,
    UpdateOrganizationRequest,
};

use crate::error::{ApiJson, AppError};
use crate::middleware::auth::Principal;
use crate::services::org_service::OrgService;
use crate::state::AppState;

/// `GET /api/v1/orgs` – every organization the caller belongs to.
pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<OrganizationSummary>>, AppError> {
    Ok(Json(
        OrgService::list_for_principal(&state, &principal).await?,
    ))
}

/// `GET /api/v1/orgs/{org_id}`
pub async fn get(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<OrganizationSummary>, AppError> {
    Ok(Json(
        OrgService::get_for_principal(&state, &principal, &org_id).await?,
    ))
}

/// `POST /api/v1/orgs` – create a team organization; the caller becomes owner.
pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateOrganizationRequest>,
) -> Result<Json<OrganizationSummary>, AppError> {
    Ok(Json(
        OrgService::create_team(&state, &principal, &request.name).await?,
    ))
}

/// `PATCH /api/v1/orgs/{org_id}` – rename. Personal spaces may be renamed.
pub async fn rename(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<UpdateOrganizationRequest>,
) -> Result<Json<OrganizationSummary>, AppError> {
    Ok(Json(
        OrgService::rename(&state, &principal, &org_id, &request.name).await?,
    ))
}

/// `DELETE /api/v1/orgs/{org_id}` – soft delete. Machines are never deleted.
pub async fn delete(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    OrgService::delete(&state, &principal, &org_id).await?;
    Ok(StatusCode::ACCEPTED)
}

/// `GET /api/v1/orgs/{org_id}/members`
pub async fn members(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<OrgMember>>, AppError> {
    Ok(Json(
        OrgService::members(&state, &principal, &org_id).await?,
    ))
}

/// `POST /api/v1/orgs/{org_id}/members` – add an existing account by email.
pub async fn add_member(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<AddMemberRequest>,
) -> Result<Json<Membership>, AppError> {
    Ok(Json(
        OrgService::add_member(&state, &principal, &org_id, &request).await?,
    ))
}

/// `PATCH /api/v1/orgs/{org_id}/members/{user_id}` – change a role.
pub async fn update_member(
    State(state): State<AppState>,
    Path((org_id, user_id)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<UpdateMemberRequest>,
) -> Result<Json<Membership>, AppError> {
    Ok(Json(
        OrgService::update_member(&state, &principal, &org_id, &user_id, request.role).await?,
    ))
}

/// `DELETE /api/v1/orgs/{org_id}/members/{user_id}`
pub async fn remove_member(
    State(state): State<AppState>,
    Path((org_id, user_id)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    OrgService::remove_member(&state, &principal, &org_id, &user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/orgs/{org_id}/leave` – leave a team organization.
pub async fn leave(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    OrgService::leave(&state, &principal, &org_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/orgs/{org_id}/invitations`
pub async fn invitations(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<OrgInvitation>>, AppError> {
    Ok(Json(
        OrgService::invitations(&state, &principal, &org_id).await?,
    ))
}

/// `POST /api/v1/orgs/{org_id}/invitations` – create a one-time invite token.
pub async fn create_invitation(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<CreateInvitationRequest>,
) -> Result<Json<CreatedOrgInvitation>, AppError> {
    Ok(Json(
        OrgService::create_invitation(&state, &principal, &org_id, &request).await?,
    ))
}

/// `GET /api/v1/admin/unowned-machines` – machines whose organization was
/// deleted. Operational recovery only; scoped to the bootstrap admin.
pub async fn unowned_machines(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<String>>, AppError> {
    if !principal.is_legacy_admin() {
        return Err(AppError::forbidden(
            "only the bootstrap admin token may list unowned machines",
        ));
    }
    Ok(Json(OrgService::unowned_machines(&state).await?))
}

/// Body of `POST /api/v1/admin/claim-machine`.
#[derive(Debug, serde::Deserialize)]
pub struct ClaimMachineRequest {
    /// Machine that lost its organization.
    pub machine_id: String,
    /// Organization that should own it.
    pub org_id: String,
}

/// `POST /api/v1/admin/claim-machine` – assign an unowned machine.
pub async fn claim_machine(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    ApiJson(request): ApiJson<ClaimMachineRequest>,
) -> Result<StatusCode, AppError> {
    OrgService::claim_machine(&state, &principal, &request.machine_id, &request.org_id).await?;
    state
        .event_index
        .set_machine_org(&request.machine_id, &request.org_id);
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/orgs/{org_id}/invitations/{invitation_id}`
pub async fn revoke_invitation(
    State(state): State<AppState>,
    Path((org_id, invitation_id)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, AppError> {
    OrgService::revoke_invitation(&state, &principal, &org_id, &invitation_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

//! Pairing-code creation.

use chrono::Duration;
use fleet_core_auth::{generate_pair_code, hash_pair_code};
use fleet_core_common::now;
use fleet_core_protocol::authz::DEFAULT_ORG_ID;
use fleet_core_protocol::pairing::{CreatePairingCodeRequest, PairingCodeResponse};

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::state::AppState;

/// Business logic for pairing codes.
pub struct PairingService;

impl PairingService {
    /// Create a pairing code scoped to the caller's organization.
    pub async fn create_code(
        state: &AppState,
        principal: &Principal,
        request: CreatePairingCodeRequest,
    ) -> Result<PairingCodeResponse, AppError> {
        let org_id = match principal.org_id() {
            Some(org) => {
                if let Some(requested) = request.org_id.as_deref() {
                    if requested != org {
                        return Err(AppError::forbidden(
                            "cannot create pairing codes outside your organization",
                        ));
                    }
                }
                org.to_string()
            }
            // The bootstrap admin token has no active organization and must
            // name one explicitly.
            None => request
                .org_id
                .clone()
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| DEFAULT_ORG_ID.to_string()),
        };

        let organization = state
            .db
            .organizations
            .get(&org_id)
            .await?
            .filter(|organization| organization.deleted_at.is_none())
            .ok_or_else(|| AppError::not_found(format!("organization {org_id} not found")))?;

        let code = generate_pair_code();
        let code_hash = hash_pair_code(&code)
            .ok_or_else(|| AppError::internal("generated pairing code failed validation"))?;
        let expires_at = now() + Duration::seconds(state.config.auth.pair_code_ttl_secs as i64);
        let id = state
            .db
            .pairing_codes
            .create_audited(
                &organization.id,
                &principal.actor,
                request.machine_name.as_deref(),
                expires_at,
                &code_hash,
            )
            .await?;

        Ok(PairingCodeResponse {
            id,
            code,
            expires_at,
            org_id: organization.id,
        })
    }
}

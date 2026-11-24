#![allow(clippy::result_large_err)]

//! Organization and membership domain logic.
//!
//! Products of this module:
//!
//! - a user always has a personal organization, created lazily on first sign-in;
//! - team organizations are created by any signed-in user, who becomes owner;
//! - membership changes never escalate a role the caller could not hold;
//! - the last owner can never be removed, demoted or leave;
//! - deleting an organization is soft first and never deletes machines.

use chrono::Duration;
use fleet_core_auth::generate_pair_code;
use fleet_core_common::now;
use fleet_core_protocol::authz::{
    AddMemberRequest, CreateInvitationRequest, CreatedOrgInvitation, Membership, OrgInvitation,
    OrgMember, Organization, OrganizationKind, OrganizationSummary, UserRole,
};

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::policy::Policy;
use crate::state::AppState;

/// Invitation lifetime when none is configured.
const DEFAULT_INVITATION_TTL_HOURS: i64 = 72;

/// Organization-facing operations.
pub struct OrgService;

impl OrgService {
    /// Every organization the principal belongs to, personal space included.
    pub async fn list_for_principal(
        state: &AppState,
        principal: &Principal,
    ) -> Result<Vec<OrganizationSummary>, AppError> {
        if principal.is_legacy_admin() {
            let mut summaries = Vec::new();
            for organization in state.db.organizations.list().await? {
                summaries.push(OrganizationSummary {
                    organization,
                    role: UserRole::Owner,
                });
            }
            return Ok(summaries);
        }
        let mut summaries = Vec::new();
        for membership in principal.memberships() {
            if let Some(organization) = state.db.organizations.get(&membership.org_id).await? {
                if organization.deleted_at.is_some() {
                    continue;
                }
                summaries.push(OrganizationSummary {
                    organization,
                    role: membership.role,
                });
            }
        }
        summaries.sort_by(|left, right| {
            // Personal space first, then team organizations by name.
            let left_personal = left.organization.kind == OrganizationKind::Personal;
            let right_personal = right.organization.kind == OrganizationKind::Personal;
            right_personal
                .cmp(&left_personal)
                .then_with(|| left.organization.name.cmp(&right.organization.name))
        });
        Ok(summaries)
    }

    /// Fetch one organization the principal belongs to.
    pub async fn get_for_principal(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
    ) -> Result<OrganizationSummary, AppError> {
        let role = super::acl::ensure_org_member(principal, org_id)?;
        let organization = state
            .db
            .organizations
            .get(org_id)
            .await?
            .ok_or_else(|| AppError::not_found(format!("organization {org_id} not found")))?;
        Ok(OrganizationSummary { organization, role })
    }

    /// Create the personal organization of a user if it does not exist yet.
    pub async fn ensure_personal(
        state: &AppState,
        user_id: &str,
        display_name: &str,
    ) -> Result<Organization, AppError> {
        if let Some(organization) = state.db.organizations.personal_for_user(user_id).await? {
            return Ok(organization);
        }
        let organization = state
            .db
            .organizations
            .create(OrganizationKind::Personal, display_name, Some(user_id))
            .await?;
        state
            .db
            .memberships
            .upsert(&organization.id, user_id, UserRole::Owner, None)
            .await?;
        Ok(organization)
    }

    /// Create a team organization owned by the caller.
    ///
    /// The bootstrap admin token has no user, so it may create an ownerless
    /// team organization during initial setup; the first member added with the
    /// owner role becomes its recorded owner.
    pub async fn create_team(
        state: &AppState,
        principal: &Principal,
        name: &str,
    ) -> Result<OrganizationSummary, AppError> {
        let name = validate_org_name(name)?;
        let owner = principal.user();
        if let Some(user) = owner {
            // A user without a personal organization has never completed
            // sign-in; creating one here keeps "every user has a personal
            // space" true for every organization endpoint.
            Self::ensure_personal(state, &user.id, &user.name).await?;
        }

        let organization = state
            .db
            .organizations
            .create(
                OrganizationKind::Team,
                &name,
                owner.map(|user| user.id.as_str()),
            )
            .await?;
        if let Some(user) = owner {
            state
                .db
                .memberships
                .upsert(&organization.id, &user.id, UserRole::Owner, None)
                .await?;
        }
        audit(
            state,
            principal,
            Some(&organization.id),
            "org.create",
            &organization.id,
            serde_json::json!({"name": organization.name, "kind": organization.kind.as_str()}),
        )
        .await;
        Ok(OrganizationSummary {
            organization,
            role: UserRole::Owner,
        })
    }

    /// Rename an organization. Personal spaces are renameable and cannot be
    /// converted into teams.
    pub async fn rename(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
        name: &str,
    ) -> Result<OrganizationSummary, AppError> {
        let name = validate_org_name(name)?;
        // Membership is checked first so a non-member gets the same 404 as for
        // any other invisible resource instead of a role error.
        super::acl::ensure_org_member(principal, org_id)?;
        super::acl::ensure_owner(principal)?;
        let organization = state
            .db
            .organizations
            .get(org_id)
            .await?
            .ok_or_else(|| AppError::not_found(format!("organization {org_id} not found")))?;
        if organization.deleted_at.is_some() {
            return Err(AppError::not_found(format!(
                "organization {org_id} not found"
            )));
        }
        if !state.db.organizations.rename(org_id, &name).await? {
            return Err(AppError::not_found(format!(
                "organization {org_id} not found"
            )));
        }
        audit(
            state,
            principal,
            Some(org_id),
            "org.rename",
            org_id,
            serde_json::json!({"from": organization.name, "to": name}),
        )
        .await;
        Self::get_for_principal(state, principal, org_id).await
    }

    /// Soft-delete an organization. Machines become unowned, never deleted.
    pub async fn delete(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
    ) -> Result<(), AppError> {
        // Membership first, for the same 404-not-403 reason as `rename`.
        super::acl::ensure_org_member(principal, org_id)?;
        super::acl::ensure_owner(principal)?;
        let organization = state
            .db
            .organizations
            .get(org_id)
            .await?
            .ok_or_else(|| AppError::not_found(format!("organization {org_id} not found")))?;
        if organization.kind == OrganizationKind::Personal {
            return Err(AppError::bad_request(
                "the personal organization cannot be deleted",
            ));
        }
        // Refuse while machines are connected so a delete cannot silently
        // orphan live daemons. The operator moves them first.
        let machines = state.db.machine_orgs.ids_for_org(org_id).await?;
        if !machines.is_empty() {
            return Err(AppError::conflict(format!(
                "move or revoke {} machine(s) before deleting this organization",
                machines.len()
            )));
        }
        if !state.db.organizations.soft_delete(org_id).await? {
            return Err(AppError::not_found(format!(
                "organization {org_id} not found"
            )));
        }
        audit(
            state,
            principal,
            Some(org_id),
            "org.delete",
            org_id,
            serde_json::json!({"name": organization.name}),
        )
        .await;
        Ok(())
    }

    /// Members of one organization. Emails are exposed to administrators only.
    pub async fn members(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
    ) -> Result<Vec<OrgMember>, AppError> {
        let role = super::acl::ensure_org_member(principal, org_id)?;
        if !Policy::allows(role, super::policy::Action::Read) {
            return Err(AppError::forbidden("you may not read this organization"));
        }
        let expose_emails = role.is_admin();
        let mut members = state.db.memberships.members_of(org_id).await?;
        if !expose_emails {
            for member in &mut members {
                member.email = None;
            }
        }
        Ok(members)
    }

    /// Add an existing account to an organization by email.
    pub async fn add_member(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
        request: &AddMemberRequest,
    ) -> Result<Membership, AppError> {
        super::acl::ensure_admin(principal)?;
        let role = super::acl::ensure_org_member(principal, org_id)?;
        ensure_can_assign(role, request.role)?;
        let email = normalize_email(&request.email)?;
        let user = state
            .db
            .users
            .by_email(&email)
            .await?
            .ok_or_else(|| AppError::not_found(format!("no account for {email}")))?;
        if state.db.memberships.get(org_id, &user.id).await?.is_some() {
            return Err(AppError::conflict(format!(
                "{email} is already a member of this organization"
            )));
        }
        let membership = state
            .db
            .memberships
            .upsert(
                org_id,
                &user.id,
                request.role,
                principal.user().map(|u| u.id.as_str()),
            )
            .await?;
        Self::sync_owner(state, org_id, &user.id, request.role).await?;
        audit(
            state,
            principal,
            Some(org_id),
            "org.member.add",
            &user.id,
            serde_json::json!({"email": email, "role": request.role.as_str()}),
        )
        .await;
        Ok(membership)
    }

    /// Change a member's role.
    pub async fn update_member(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
        user_id: &str,
        role: UserRole,
    ) -> Result<Membership, AppError> {
        super::acl::ensure_admin(principal)?;
        let caller_role = super::acl::ensure_org_member(principal, org_id)?;
        ensure_can_assign(caller_role, role)?;
        let existing = state
            .db
            .memberships
            .get(org_id, user_id)
            .await?
            .ok_or_else(|| AppError::not_found(format!("member {user_id} not found")))?;
        if existing.role == role {
            return Ok(existing);
        }
        // Demoting the last owner would leave the organization ownerless.
        if existing.role.is_owner()
            && state
                .db
                .memberships
                .count_role(org_id, UserRole::Owner)
                .await?
                <= 1
        {
            return Err(AppError::conflict(
                "an organization must keep at least one owner",
            ));
        }
        let membership = state
            .db
            .memberships
            .upsert(org_id, user_id, role, None)
            .await?;
        Self::sync_owner(state, org_id, user_id, role).await?;
        audit(
            state,
            principal,
            Some(org_id),
            "org.member.role",
            user_id,
            serde_json::json!({"from": existing.role.as_str(), "to": role.as_str()}),
        )
        .await;
        Ok(membership)
    }

    /// Remove a member. The last owner cannot be removed.
    pub async fn remove_member(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
        user_id: &str,
    ) -> Result<(), AppError> {
        super::acl::ensure_admin(principal)?;
        let caller_role = super::acl::ensure_org_member(principal, org_id)?;
        let existing = state
            .db
            .memberships
            .get(org_id, user_id)
            .await?
            .ok_or_else(|| AppError::not_found(format!("member {user_id} not found")))?;
        if existing.role.is_owner() {
            if !caller_role.is_owner() {
                return Err(AppError::forbidden("only an owner may remove an owner"));
            }
            if state
                .db
                .memberships
                .count_role(org_id, UserRole::Owner)
                .await?
                <= 1
            {
                return Err(AppError::conflict(
                    "an organization must keep at least one owner",
                ));
            }
        }
        if !state.db.memberships.remove(org_id, user_id).await? {
            return Err(AppError::not_found(format!("member {user_id} not found")));
        }
        audit(
            state,
            principal,
            Some(org_id),
            "org.member.remove",
            user_id,
            serde_json::json!({"role": existing.role.as_str()}),
        )
        .await;
        Ok(())
    }

    /// Leave an organization. The last owner must transfer ownership first.
    pub async fn leave(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
    ) -> Result<(), AppError> {
        let user = principal.user().ok_or_else(|| {
            AppError::forbidden("the bootstrap token cannot leave an organization")
        })?;
        let role = super::acl::ensure_org_member(principal, org_id)?;
        let organization = state
            .db
            .organizations
            .get(org_id)
            .await?
            .ok_or_else(|| AppError::not_found(format!("organization {org_id} not found")))?;
        if organization.kind == OrganizationKind::Personal {
            return Err(AppError::bad_request(
                "the personal organization cannot be left",
            ));
        }
        if role.is_owner()
            && state
                .db
                .memberships
                .count_role(org_id, UserRole::Owner)
                .await?
                <= 1
        {
            return Err(AppError::conflict(
                "transfer ownership before leaving this organization",
            ));
        }
        state.db.memberships.remove(org_id, &user.id).await?;
        audit(
            state,
            principal,
            Some(org_id),
            "org.member.leave",
            &user.id,
            serde_json::json!({}),
        )
        .await;
        Ok(())
    }

    /// Invite an email address that has no account yet.
    pub async fn create_invitation(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
        request: &CreateInvitationRequest,
    ) -> Result<CreatedOrgInvitation, AppError> {
        super::acl::ensure_admin(principal)?;
        let role = super::acl::ensure_org_member(principal, org_id)?;
        ensure_can_assign(role, request.role)?;
        let email = normalize_email(&request.email)?;
        if state.db.users.by_email(&email).await?.is_some() {
            return Err(AppError::conflict(
                "an account already exists for this email; add it as a member instead",
            ));
        }
        if state.db.invitations.pending_exists(org_id, &email).await? {
            return Err(AppError::conflict(
                "an invitation for this email is already pending",
            ));
        }
        let token = generate_pair_code();
        let token_hash = fleet_core_auth::hash_pair_code(&token)
            .ok_or_else(|| AppError::internal("failed to hash invitation token"))?;
        let invitation = state
            .db
            .invitations
            .create(
                org_id,
                &email,
                request.role,
                &token_hash,
                principal
                    .user()
                    .map(|user| user.id.as_str())
                    .unwrap_or("admin"),
                now() + Duration::hours(DEFAULT_INVITATION_TTL_HOURS),
            )
            .await?;
        audit(
            state,
            principal,
            Some(org_id),
            "org.invite.create",
            &invitation.id,
            serde_json::json!({"email": email, "role": request.role.as_str()}),
        )
        .await;
        Ok(CreatedOrgInvitation { invitation, token })
    }

    /// List pending invitations.
    pub async fn invitations(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
    ) -> Result<Vec<OrgInvitation>, AppError> {
        super::acl::ensure_admin(principal)?;
        super::acl::ensure_org_member(principal, org_id)?;
        Ok(state.db.invitations.pending_for_org(org_id).await?)
    }

    /// Revoke a pending invitation.
    pub async fn revoke_invitation(
        state: &AppState,
        principal: &Principal,
        org_id: &str,
        invitation_id: &str,
    ) -> Result<(), AppError> {
        super::acl::ensure_admin(principal)?;
        super::acl::ensure_org_member(principal, org_id)?;
        if !state.db.invitations.revoke(invitation_id).await? {
            return Err(AppError::not_found("invitation not found"));
        }
        audit(
            state,
            principal,
            Some(org_id),
            "org.invite.revoke",
            invitation_id,
            serde_json::json!({}),
        )
        .await;
        Ok(())
    }

    /// Accept every pending invitation bound to a verified email.
    ///
    /// Called from the OAuth callback for a new or returning account. It is
    /// idempotent: an invitation is only consumed once.
    pub async fn accept_invitations_for_email(
        state: &AppState,
        user_id: &str,
        email: &str,
    ) -> Result<Vec<Membership>, AppError> {
        let mut accepted = Vec::new();
        for invitation in state.db.invitations.pending_for_email(email).await? {
            // Bind the invitation to the accepting account. The email already
            // matches, so this cannot move a membership to a different person.
            let membership = state
                .db
                .memberships
                .upsert(
                    &invitation.org_id,
                    user_id,
                    invitation.role,
                    Some(&invitation.created_by),
                )
                .await?;
            // Mark accepted through the storage token path. The invitation row
            // is looked up by id during the update.
            state.db.invitations.mark_accepted(&invitation.id).await?;
            accepted.push(membership);
        }
        Ok(accepted)
    }

    /// Transfer a machine to another organization.
    ///
    /// Both the source and the target must grant `ManageMachines` to the
    /// caller, and the caller must be a member of both.
    pub async fn transfer_machine(
        state: &AppState,
        principal: &Principal,
        machine_id: &str,
        target_org: &str,
    ) -> Result<(), AppError> {
        super::acl::ensure_machine_admin(state, principal, machine_id).await?;
        let target_role = super::acl::ensure_org_member(principal, target_org)?;
        if !Policy::allows(target_role, super::policy::Action::ManageMachines) {
            return Err(AppError::forbidden(
                "you may not manage machines in the target organization",
            ));
        }
        let source = state.db.machine_orgs.get(machine_id).await?;
        if source.as_deref() == Some(target_org) {
            return Ok(());
        }
        state.db.machine_orgs.assign(machine_id, target_org).await?;
        audit(
            state,
            principal,
            Some(target_org),
            "machine.transfer",
            machine_id,
            serde_json::json!({"from": source, "to": target_org}),
        )
        .await;
        Ok(())
    }

    /// Claim a machine that lost its organization (e.g. the owner organization
    /// was deleted). Only the bootstrap admin may do this.
    pub async fn claim_machine(
        state: &AppState,
        principal: &Principal,
        machine_id: &str,
        target_org: &str,
    ) -> Result<(), AppError> {
        if !principal.is_legacy_admin() {
            return Err(AppError::forbidden(
                "only the bootstrap admin token may claim an unowned machine",
            ));
        }
        state
            .db
            .organizations
            .get(target_org)
            .await?
            .ok_or_else(|| AppError::not_found(format!("organization {target_org} not found")))?;
        if state.db.machine_orgs.get(machine_id).await?.is_some() {
            return Err(AppError::conflict(
                "this machine still belongs to an organization",
            ));
        }
        state.db.machine_orgs.assign(machine_id, target_org).await?;
        audit(
            state,
            principal,
            Some(target_org),
            "machine.claim",
            machine_id,
            serde_json::json!({"to": target_org}),
        )
        .await;
        Ok(())
    }

    /// Machines that belong to no organization.
    pub async fn unowned_machines(state: &AppState) -> Result<Vec<String>, AppError> {
        Ok(state.db.machine_orgs.unowned().await?)
    }

    /// Keep the denormalized `organizations.owner_user_id` in sync with the
    /// owner memberships. The column exists so the UI can show the owner
    /// without a join and so an ownerless organization (created by the
    /// bootstrap token) can be adopted by the first owner.
    async fn sync_owner(
        state: &AppState,
        org_id: &str,
        user_id: &str,
        role: UserRole,
    ) -> Result<(), AppError> {
        if !role.is_owner() {
            return Ok(());
        }
        let organization = state.db.organizations.get(org_id).await?;
        if organization.is_some_and(|organization| organization.owner_user_id.is_none()) {
            state
                .db
                .organizations
                .set_owner(org_id, Some(user_id))
                .await?;
        }
        Ok(())
    }
}

fn validate_org_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("organization name must not be empty"));
    }
    if name.len() > 120 {
        return Err(AppError::bad_request(
            "organization name is limited to 120 characters",
        ));
    }
    Ok(name.to_string())
}

fn normalize_email(email: &str) -> Result<String, AppError> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') || email.len() > 320 {
        return Err(AppError::bad_request("a valid email address is required"));
    }
    Ok(email)
}

fn ensure_can_assign(caller: UserRole, target: UserRole) -> Result<(), AppError> {
    if Policy::can_assign(caller, target) {
        Ok(())
    } else {
        Err(AppError::forbidden(format!(
            "role '{}' may not grant '{}'",
            caller.as_str(),
            target.as_str()
        )))
    }
}

async fn audit(
    state: &AppState,
    principal: &Principal,
    org_id: Option<&str>,
    action: &str,
    subject: &str,
    details: serde_json::Value,
) {
    if let Err(error) = state
        .db
        .audit
        .append(&principal.actor, org_id, action, subject, details)
        .await
    {
        tracing::warn!(error = %error, action, "failed to audit organization action");
    }
}

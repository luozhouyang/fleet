//! Organization, user and role contracts for multi-user access control.
//!
//! A user is global: it has an email, a display name and credentials, but no
//! organization. Membership in an organization is expressed by [`Membership`],
//! so one user can belong to several organizations with a different role in
//! each. A machine, workspace or agent session still belongs to exactly one
//! organization.

use serde::{Deserialize, Serialize};

use crate::common::{OrganizationId, Timestamp, UserId};

/// Organization that owns machines and users created without an explicit org.
pub const DEFAULT_ORG_ID: &str = "org_default";

/// Role assigned to a user inside one organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    /// Sole owner of an organization; can transfer ownership and delete it.
    Owner,
    /// Manage members, machines, users and all Git writes.
    Admin,
    /// Use machines, workspaces, sessions and Git writes.
    Member,
    /// Read-only access to machines, workspaces and sessions.
    Viewer,
}

impl UserRole {
    /// Parse the stored/database representation.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    /// Stable database and API representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    /// Whether the role may perform write operations.
    pub fn can_write(&self) -> bool {
        !matches!(self, Self::Viewer)
    }

    /// Whether the role may manage users, organizations and machines.
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether the role may transfer ownership and delete the organization.
    pub fn is_owner(&self) -> bool {
        matches!(self, Self::Owner)
    }

    /// Whether the role may grant or revoke [`UserRole::Owner`].
    pub fn can_grant_owner(&self) -> bool {
        self.is_owner()
    }

    /// Ordering used to reject privilege escalation: higher is more powerful.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Owner => 3,
            Self::Admin => 2,
            Self::Member => 1,
            Self::Viewer => 0,
        }
    }

    /// Whether `self` may assign `target` to another member.
    ///
    /// Administrators may manage every role except ownership, so they can
    /// promote a peer to `admin` without being able to mint an owner. Only an
    /// owner may hand out ownership.
    pub fn can_assign(&self, target: UserRole) -> bool {
        match self {
            Self::Owner => true,
            Self::Admin => !target.is_owner(),
            _ => false,
        }
    }
}

/// Whether an organization is a user's private space or a shared team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationKind {
    /// Automatically created private space; exactly one per user.
    Personal,
    /// Shared organization created by a user.
    Team,
}

impl OrganizationKind {
    /// Parse the stored/database representation.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "personal" => Some(Self::Personal),
            "team" => Some(Self::Team),
            _ => None,
        }
    }

    /// Stable database and API representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Team => "team",
        }
    }
}

/// One organization. Machines and memberships belong to an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    /// Stable id, e.g. `org_...`.
    pub id: OrganizationId,
    /// Display name.
    pub name: String,
    /// Personal or team.
    #[serde(default = "default_org_kind")]
    pub kind: OrganizationKind,
    /// Owner for team organizations; `None` for a personal space is never used
    /// because the creator is recorded here as well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<UserId>,
    /// Set while the organization is pending deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification time.
    #[serde(default)]
    pub updated_at: Timestamp,
}

fn default_org_kind() -> OrganizationKind {
    OrganizationKind::Team
}

/// An organization plus the caller's role in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationSummary {
    /// The organization.
    pub organization: Organization,
    /// Caller's role inside the organization.
    pub role: UserRole,
}

/// Membership of one user in one organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// Organization.
    pub org_id: OrganizationId,
    /// Member.
    pub user_id: UserId,
    /// Role inside the organization.
    pub role: UserRole,
    /// When the membership was created.
    pub created_at: Timestamp,
    /// Last role change.
    pub updated_at: Timestamp,
    /// User who invited this member, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invited_by: Option<UserId>,
}

/// A member as shown in the organization member list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgMember {
    /// Member user id.
    pub user_id: UserId,
    /// Verified email, when the account has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name.
    pub name: String,
    /// Avatar URL, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Role inside the organization.
    pub role: UserRole,
    /// When the user joined.
    pub created_at: Timestamp,
    /// Last role change.
    pub updated_at: Timestamp,
}

/// A pending invitation. Delivery is out of band; the API returns a one-time
/// token that the inviter shares with the invitee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgInvitation {
    /// Invitation id.
    pub id: String,
    /// Target organization.
    pub org_id: OrganizationId,
    /// Email the invitation is bound to.
    pub email: String,
    /// Role granted on acceptance.
    pub role: UserRole,
    /// Who created it.
    pub created_by: UserId,
    /// Creation time.
    pub created_at: Timestamp,
    /// Expiry time.
    pub expires_at: Timestamp,
    /// Set once the invitee accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<Timestamp>,
}

/// A newly created invitation plus its one-time plaintext token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedOrgInvitation {
    /// Persisted invitation.
    pub invitation: OrgInvitation,
    /// Plaintext token; shown exactly once.
    pub token: String,
}

/// A user without secrets. Session tokens are only returned when created or
/// rotated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// Stable id, e.g. `user_...`.
    pub id: UserId,
    /// Verified email address; the account key for invitations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name; also used as the audit actor.
    pub name: String,
    /// Avatar URL supplied by the identity provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification time.
    pub updated_at: Timestamp,
    /// When the current bearer token was issued.
    #[serde(default)]
    pub token_issued_at: Option<Timestamp>,
    /// When the current bearer token expires; `None` means never.
    #[serde(default)]
    pub token_expires_at: Option<Timestamp>,
    /// Set when the account is disabled; all sessions are revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<Timestamp>,
}

/// How the current API caller authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// The legacy shared admin token; used only to bootstrap the deployment.
    LegacyAdmin,
    /// A persisted user with memberships.
    User,
}

/// Identity of the current API caller, scoped to the active organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    /// Authentication kind.
    pub kind: PrincipalKind,
    /// Persisted user for user principals.
    #[serde(default)]
    pub user: Option<User>,
    /// Active organization; absent only for the legacy admin.
    #[serde(default)]
    pub org_id: Option<OrganizationId>,
    /// Effective role in the active organization.
    pub role: UserRole,
}

/// Create a team organization. The caller becomes its owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOrganizationRequest {
    /// Display name.
    pub name: String,
}

/// Rename an organization. Personal spaces may be renamed without conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOrganizationRequest {
    /// New display name.
    pub name: String,
}

/// Add an existing user to an organization by email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddMemberRequest {
    /// Verified email of an existing account.
    pub email: String,
    /// Role to grant.
    pub role: UserRole,
}

/// Change a member's role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateMemberRequest {
    /// New role.
    pub role: UserRole,
}

/// Invite an email address that has no account yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInvitationRequest {
    /// Email the invitation is bound to.
    pub email: String,
    /// Role granted on acceptance.
    pub role: UserRole,
}

/// Create a user account. Used by administrators who do not want to wait for an
/// OAuth sign-in; the bearer token is returned once for CLI use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateUserRequest {
    /// Verified email for the new account.
    pub email: String,
    /// Display name.
    pub name: String,
    /// Initial organization. Defaults to the caller's active organization.
    #[serde(default)]
    pub org_id: Option<OrganizationId>,
    /// Role inside that organization.
    pub role: UserRole,
    /// Token lifetime override in days; `0` means never, omitted uses the default.
    #[serde(default)]
    pub token_expires_in_days: Option<u64>,
}

/// User plus the generated bearer token. The token is shown exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedUser {
    /// The persisted user.
    pub user: User,
    /// Membership created with the user.
    pub membership: Membership,
    /// Plaintext bearer token; never stored.
    pub token: String,
}

/// Rotate a user's bearer token. The old token stops working immediately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotatedUserToken {
    /// User whose token was rotated.
    pub user_id: UserId,
    /// New plaintext bearer token; never stored.
    pub token: String,
    /// When the new token expires; `None` means never.
    #[serde(default)]
    pub token_expires_at: Option<Timestamp>,
}

/// Assign a machine to an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignMachineRequest {
    /// Target organization.
    pub org_id: OrganizationId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;

    fn sample_timestamp() -> Timestamp {
        crate::common::now()
    }

    fn sample_user(timestamp: Timestamp) -> User {
        User {
            id: "user_1".into(),
            email: Some("ada@example.com".into()),
            name: "Ada".into(),
            avatar_url: None,
            created_at: timestamp,
            updated_at: timestamp,
            token_issued_at: Some(timestamp),
            token_expires_at: None,
            disabled_at: None,
        }
    }

    #[test]
    fn user_token_lifetime_fields_round_trip() {
        let issued = sample_timestamp();
        let expires = issued + Duration::days(30);
        let mut user = sample_user(issued);
        user.token_expires_at = Some(expires);

        let encoded = serde_json::to_value(&user).unwrap();
        let encoded_issued: Timestamp =
            serde_json::from_value(encoded["token_issued_at"].clone()).unwrap();
        let encoded_expires: Timestamp =
            serde_json::from_value(encoded["token_expires_at"].clone()).unwrap();
        assert_eq!(encoded_issued, issued);
        assert_eq!(encoded_expires, expires);

        let decoded: User = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, user);
    }

    #[test]
    fn omitted_optional_user_fields_default_to_none() {
        let user: User = serde_json::from_value(json!({
            "id": "user_1",
            "name": "Ada",
            "created_at": "2026-01-02T03:04:05Z",
            "updated_at": "2026-01-02T03:04:05Z"
        }))
        .unwrap();
        assert_eq!(user.token_issued_at, None);
        assert_eq!(user.token_expires_at, None);
        assert_eq!(user.email, None);
        assert_eq!(user.avatar_url, None);
        assert_eq!(user.disabled_at, None);

        let rotated: RotatedUserToken = serde_json::from_value(json!({
            "user_id": "user_1",
            "token": "fla_user_token"
        }))
        .unwrap();
        assert_eq!(rotated.token_expires_at, None);

        let request: CreateUserRequest = serde_json::from_value(json!({
            "email": "ada@example.com",
            "name": "Ada",
            "role": "member"
        }))
        .unwrap();
        assert_eq!(request.token_expires_in_days, None);
        assert_eq!(request.org_id, None);
    }

    #[test]
    fn me_reports_legacy_admin_and_user_principals() {
        let legacy = Me {
            kind: PrincipalKind::LegacyAdmin,
            user: None,
            org_id: None,
            role: UserRole::Admin,
        };
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert_eq!(encoded["kind"], "legacy_admin");
        assert_eq!(encoded["role"], "admin");
        assert!(encoded["user"].is_null());
        assert_eq!(serde_json::from_value::<Me>(encoded).unwrap(), legacy);

        let timestamp = sample_timestamp();
        let me = Me {
            kind: PrincipalKind::User,
            user: Some(sample_user(timestamp)),
            org_id: Some("org_default".into()),
            role: UserRole::Member,
        };
        let encoded = serde_json::to_value(&me).unwrap();
        assert_eq!(encoded["kind"], "user");
        assert_eq!(encoded["user"]["id"], "user_1");
        assert_eq!(encoded["org_id"], "org_default");
        assert_eq!(serde_json::from_value::<Me>(encoded).unwrap(), me);
    }

    #[test]
    fn create_request_accepts_an_explicit_expiry_in_days() {
        let request: CreateUserRequest = serde_json::from_value(json!({
            "email": "ada@example.com",
            "name": "Ada",
            "role": "admin",
            "token_expires_in_days": 0
        }))
        .unwrap();
        assert_eq!(request.token_expires_in_days, Some(0));
        assert_eq!(
            serde_json::to_value(&request).unwrap()["token_expires_in_days"],
            0
        );
    }

    #[test]
    fn roles_parse_and_rank_monotonically() {
        for role in [
            UserRole::Owner,
            UserRole::Admin,
            UserRole::Member,
            UserRole::Viewer,
        ] {
            assert_eq!(UserRole::parse(role.as_str()), Some(role));
        }
        assert!(UserRole::parse("superuser").is_none());
        assert_eq!(UserRole::parse("owner"), Some(UserRole::Owner));

        assert!(UserRole::Owner.is_admin());
        assert!(UserRole::Owner.is_owner());
        assert!(UserRole::Admin.is_admin());
        assert!(!UserRole::Member.is_admin());
        assert!(!UserRole::Viewer.can_write());

        assert!(UserRole::Owner.rank() > UserRole::Admin.rank());
        assert!(UserRole::Admin.rank() > UserRole::Member.rank());
        assert!(UserRole::Member.rank() > UserRole::Viewer.rank());
    }

    #[test]
    fn only_owners_may_grant_ownership() {
        assert!(UserRole::Owner.can_assign(UserRole::Owner));
        assert!(UserRole::Owner.can_assign(UserRole::Admin));
        assert!(!UserRole::Admin.can_assign(UserRole::Owner));
        assert!(UserRole::Admin.can_assign(UserRole::Admin));
        assert!(UserRole::Admin.can_assign(UserRole::Member));
        assert!(!UserRole::Member.can_assign(UserRole::Member));
    }

    #[test]
    fn organization_kind_round_trips() {
        assert_eq!(
            OrganizationKind::parse(OrganizationKind::Personal.as_str()),
            Some(OrganizationKind::Personal)
        );
        let organization: Organization = serde_json::from_value(json!({
            "id": "org_1",
            "name": "Ada",
            "created_at": "2026-01-02T03:04:05Z"
        }))
        .unwrap();
        assert_eq!(organization.kind, OrganizationKind::Team);
        assert_eq!(organization.owner_user_id, None);
        assert_eq!(organization.deleted_at, None);
    }
}

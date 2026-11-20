//! Organization, user and role contracts for multi-user access control.

use serde::{Deserialize, Serialize};

use crate::common::{OrganizationId, Timestamp, UserId};

/// Organization that owns machines and users created without an explicit org.
pub const DEFAULT_ORG_ID: &str = "org_default";

/// Role assigned to a user inside one organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    /// Manage users, organizations, machines and all Git writes.
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
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    /// Stable database and API representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    /// Whether the role may perform write operations.
    pub fn can_write(&self) -> bool {
        !matches!(self, Self::Viewer)
    }

    /// Whether the role may manage users and organizations.
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Admin)
    }
}

/// One organization. Machines and users belong to an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    /// Stable id, e.g. `org_...`.
    pub id: OrganizationId,
    /// Display name.
    pub name: String,
    /// Creation time.
    pub created_at: Timestamp,
}

/// A user without secrets. Tokens are only returned when created/rotated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// Stable id, e.g. `user_...`.
    pub id: UserId,
    /// Organization the user belongs to.
    pub org_id: OrganizationId,
    /// Display name; also used as the audit actor.
    pub name: String,
    /// Role inside the organization.
    pub role: UserRole,
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
}

/// How the current API caller authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// The configured legacy shared admin token.
    LegacyAdmin,
    /// A persisted user's bearer token.
    User,
}

/// Identity of the current API caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    /// Authentication kind.
    pub kind: PrincipalKind,
    /// Persisted user for user principals.
    #[serde(default)]
    pub user: Option<User>,
    /// Organization scope; absent for the legacy admin.
    #[serde(default)]
    pub org_id: Option<OrganizationId>,
    /// Effective role.
    pub role: UserRole,
}

/// Create an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOrganizationRequest {
    /// Display name.
    pub name: String,
}

/// Create a user inside an existing organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateUserRequest {
    /// Target organization.
    pub org_id: OrganizationId,
    /// Display/audit name.
    pub name: String,
    /// Initial role.
    pub role: UserRole,
    /// Token lifetime override in days. `Some(0)` means never expires; omitted
    /// uses the configured default.
    #[serde(default)]
    pub token_expires_in_days: Option<u64>,
}

/// User plus the generated bearer token. The token is shown exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedUser {
    /// The persisted user.
    pub user: User,
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

    #[test]
    fn user_token_lifetime_fields_round_trip() {
        let issued = sample_timestamp();
        let expires = issued + Duration::days(30);
        let user = User {
            id: "user_1".into(),
            org_id: "org_default".into(),
            name: "Ada".into(),
            role: UserRole::Member,
            created_at: issued,
            updated_at: issued,
            token_issued_at: Some(issued),
            token_expires_at: Some(expires),
        };

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
    fn omitted_token_lifetime_fields_default_to_none() {
        let user: User = serde_json::from_value(json!({
            "id": "user_1",
            "org_id": "org_default",
            "name": "Ada",
            "role": "viewer",
            "created_at": "2026-01-02T03:04:05Z",
            "updated_at": "2026-01-02T03:04:05Z"
        }))
        .unwrap();
        assert_eq!(user.token_issued_at, None);
        assert_eq!(user.token_expires_at, None);

        let rotated: RotatedUserToken = serde_json::from_value(json!({
            "user_id": "user_1",
            "token": "fla_user_token"
        }))
        .unwrap();
        assert_eq!(rotated.token_expires_at, None);

        let request: CreateUserRequest = serde_json::from_value(json!({
            "org_id": "org_default",
            "name": "Ada",
            "role": "member"
        }))
        .unwrap();
        assert_eq!(request.token_expires_in_days, None);
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
        let user = User {
            id: "user_1".into(),
            org_id: "org_default".into(),
            name: "Ada".into(),
            role: UserRole::Member,
            created_at: timestamp,
            updated_at: timestamp,
            token_issued_at: Some(timestamp),
            token_expires_at: None,
        };
        let me = Me {
            kind: PrincipalKind::User,
            user: Some(user),
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
            "org_id": "org_default",
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
}

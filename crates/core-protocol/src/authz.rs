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
}

/// Assign a machine to an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignMachineRequest {
    /// Target organization.
    pub org_id: OrganizationId,
}

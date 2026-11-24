#![allow(clippy::result_large_err)]

//! Authorization policy: a capability table over organization roles.
//!
//! This module is the single place that answers "may this role do this?".
//! Routes resolve a resource to its owning organization and then ask
//! [`Policy::allows`]. The table is data, not code, so it can be tested as a
//! matrix and later replaced by a policy engine (Casbin domain RBAC) without
//! changing any endpoint.
//!
//! Organization roles are the only authorization dimension in the first
//! release. Resources are still named (`machine`, `workspace`, `session`) so a
//! per-resource grant can be added as another policy source later.

use fleet_core_protocol::authz::{UserRole, UserRole::*};

/// A capability a caller may exercise inside one organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Read machines, workspaces, sessions and their metadata.
    Read,
    /// Start, message, interrupt and close agent sessions.
    UseMachine,
    /// Perform Git writes (stage, unstage, commit, checkout).
    GitWrite,
    /// Register, pair, revoke, transfer machines and manage provider probes.
    ManageMachines,
    /// Invite, remove and re-role members below ownership.
    ManageMembers,
    /// Rename, delete, transfer ownership and change organization settings.
    ManageOrganization,
    /// Read the audit log.
    ReadAudit,
}

impl Action {
    /// Human-readable phrase used in error messages.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Read => "read this organization",
            Self::UseMachine => "use machines in this organization",
            Self::GitWrite => "write to Git repositories in this organization",
            Self::ManageMachines => "manage machines in this organization",
            Self::ManageMembers => "manage members of this organization",
            Self::ManageOrganization => "manage this organization",
            Self::ReadAudit => "read the audit log of this organization",
        }
    }
}

/// Capability table.
pub struct Policy;

impl Policy {
    /// Whether `role` may perform `action`.
    pub fn allows(role: UserRole, action: Action) -> bool {
        match action {
            Action::Read => true,
            Action::UseMachine => matches!(role, Owner | Admin | Member),
            Action::GitWrite => matches!(role, Owner | Admin | Member),
            Action::ManageMachines => matches!(role, Owner | Admin),
            Action::ManageMembers => matches!(role, Owner | Admin),
            Action::ManageOrganization => matches!(role, Owner),
            Action::ReadAudit => matches!(role, Owner | Admin),
        }
    }

    /// Every action a role may perform, for the `/me` capability list.
    // P0-B exposes this through `/me`; the policy matrix tests already use it.
    #[allow(dead_code)]
    pub fn actions(role: UserRole) -> Vec<Action> {
        [
            Action::Read,
            Action::UseMachine,
            Action::GitWrite,
            Action::ManageMachines,
            Action::ManageMembers,
            Action::ManageOrganization,
            Action::ReadAudit,
        ]
        .into_iter()
        .filter(|action| Self::allows(role, *action))
        .collect()
    }

    /// Whether the role may assign `target` to another member.
    ///
    /// Wraps [`UserRole::can_assign`] so the check lives in one place.
    pub fn can_assign(role: UserRole, target: UserRole) -> bool {
        role.can_assign(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIONS: [Action; 7] = [
        Action::Read,
        Action::UseMachine,
        Action::GitWrite,
        Action::ManageMachines,
        Action::ManageMembers,
        Action::ManageOrganization,
        Action::ReadAudit,
    ];

    #[test]
    fn every_role_can_read() {
        for role in [Owner, Admin, Member, Viewer] {
            assert!(Policy::allows(role, Action::Read));
        }
    }

    #[test]
    fn viewers_are_read_only() {
        assert!(!Policy::allows(Viewer, Action::UseMachine));
        assert!(!Policy::allows(Viewer, Action::GitWrite));
        assert!(!Policy::allows(Viewer, Action::ManageMachines));
        assert!(!Policy::allows(Viewer, Action::ManageMembers));
        assert!(!Policy::allows(Viewer, Action::ManageOrganization));
        assert!(!Policy::allows(Viewer, Action::ReadAudit));
    }

    #[test]
    fn members_use_but_do_not_administer() {
        assert!(Policy::allows(Member, Action::UseMachine));
        assert!(Policy::allows(Member, Action::GitWrite));
        assert!(!Policy::allows(Member, Action::ManageMachines));
        assert!(!Policy::allows(Member, Action::ManageMembers));
        assert!(!Policy::allows(Member, Action::ReadAudit));
    }

    #[test]
    fn only_owners_manage_the_organization() {
        assert!(Policy::allows(Owner, Action::ManageOrganization));
        assert!(!Policy::allows(Admin, Action::ManageOrganization));
        assert!(!Policy::allows(Member, Action::ManageOrganization));
    }

    #[test]
    fn admins_manage_members_and_read_audit() {
        assert!(Policy::allows(Admin, Action::ManageMembers));
        assert!(Policy::allows(Admin, Action::ManageMachines));
        assert!(Policy::allows(Admin, Action::ReadAudit));
    }

    #[test]
    fn role_capability_sets_are_monotonic() {
        // Every capability of a weaker role is also held by a stronger one.
        let grant = |role: UserRole, action: Action| Policy::allows(role, action);
        for action in ACTIONS {
            let owner = grant(Owner, action);
            let admin = grant(Admin, action);
            let member = grant(Member, action);
            let viewer = grant(Viewer, action);
            assert!(owner || !admin, "{action:?}: admin has more than owner");
            assert!(admin || !member, "{action:?}: member has more than admin");
            assert!(member || !viewer, "{action:?}: viewer has more than member");
        }
    }
}

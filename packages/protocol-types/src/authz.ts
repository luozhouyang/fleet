import type { OrganizationId, Timestamp, UserId } from './common';

/** Organization that owns machines created without an explicit organization. */
export const DEFAULT_ORG_ID = 'org_default';

/**
 * Role assigned to a user inside one organization.
 *
 * A user is a global account and joins organizations through memberships, so
 * the same account may be an owner in one organization and a viewer in another.
 */
export type UserRole = 'owner' | 'admin' | 'member' | 'viewer';

/** Whether an organization is a private space or a shared team. */
export type OrganizationKind = 'personal' | 'team';

/** How the current API caller authenticated. */
export type PrincipalKind = 'legacy_admin' | 'user';

/** One organization. Machines and memberships belong to an organization. */
export interface Organization {
  id: OrganizationId;
  name: string;
  kind: OrganizationKind;
  /** Owner of a team organization; absent while nobody holds ownership. */
  owner_user_id?: UserId | null;
  /** Set while the organization is pending deletion. */
  deleted_at?: Timestamp | null;
  created_at: Timestamp;
  updated_at: Timestamp;
}

/** An organization plus the caller's role in it. */
export interface OrganizationSummary {
  organization: Organization;
  role: UserRole;
}

/** Membership of one user in one organization. */
export interface Membership {
  org_id: OrganizationId;
  user_id: UserId;
  role: UserRole;
  created_at: Timestamp;
  updated_at: Timestamp;
  invited_by?: UserId | null;
}

/** A member as shown in the organization member list. */
export interface OrgMember {
  user_id: UserId;
  /** Verified email; hidden unless the caller administers the organization. */
  email?: string | null;
  name: string;
  avatar_url?: string | null;
  role: UserRole;
  created_at: Timestamp;
  updated_at: Timestamp;
}

/** A pending invitation for an email address with no account yet. */
export interface OrgInvitation {
  id: string;
  org_id: OrganizationId;
  email: string;
  role: UserRole;
  created_by: UserId;
  created_at: Timestamp;
  expires_at: Timestamp;
  accepted_at?: Timestamp | null;
}

/** A newly created invitation plus its one-time plaintext token. */
export interface CreatedOrgInvitation {
  invitation: OrgInvitation;
  token: string;
}

/** A user without secrets. Tokens are only returned when created or rotated. */
export interface User {
  id: UserId;
  /** Verified email address; the account key for invitations. */
  email?: string | null;
  name: string;
  avatar_url?: string | null;
  created_at: Timestamp;
  updated_at: Timestamp;
  /** When the current bearer token was issued. */
  token_issued_at?: Timestamp | null;
  /** When the current bearer token expires; `null` means never. */
  token_expires_at?: Timestamp | null;
  /** Set when the account is disabled; all sessions are revoked. */
  disabled_at?: Timestamp | null;
}

/** User plus the generated bearer token. The token is shown exactly once. */
export interface CreatedUser {
  user: User;
  /** Membership created together with the account. */
  membership: Membership;
  token: string;
}

/** Result of rotating a user token. The old token stops working immediately. */
export interface RotatedUserToken {
  user_id: UserId;
  token: string;
  /** New expiry; `null` means the token never expires. */
  token_expires_at?: Timestamp | null;
}

/** Identity of the current API caller, scoped to the active organization. */
export interface Me {
  kind: PrincipalKind;
  user?: User | null;
  /** Active organization; absent only for the legacy admin. */
  org_id?: OrganizationId | null;
  role: UserRole;
}

/** Create a team organization. The caller becomes its owner. */
export interface CreateOrganizationRequest {
  name: string;
}

/** Rename an organization. Personal spaces may be renamed. */
export interface UpdateOrganizationRequest {
  name: string;
}

/** Add an existing account to an organization by email. */
export interface AddMemberRequest {
  email: string;
  role: UserRole;
}

/** Change a member's role. */
export interface UpdateMemberRequest {
  role: UserRole;
}

/** Invite an email address that has no account yet. */
export interface CreateInvitationRequest {
  email: string;
  role: UserRole;
}

/** Create a user account and its first membership. */
export interface CreateUserRequest {
  /** Verified email for the new account. */
  email: string;
  /** Display name. */
  name: string;
  /** Initial organization; defaults to the caller's active organization. */
  org_id?: OrganizationId | null;
  /** Role inside that organization. */
  role: UserRole;
  /** Token lifetime override in days; `0` means never, omitted uses the default. */
  token_expires_in_days?: number | null;
}

/** Assign a machine to an organization. */
export interface AssignMachineRequest {
  org_id: OrganizationId;
}

/** Single-use ticket handed to the browser for a WebSocket upgrade. */
export interface RealtimeTicket {
  ticket: string;
  /** Organization the ticket is bound to. */
  org_id: OrganizationId;
  /** Seconds until the ticket expires. */
  expires_in: number;
}

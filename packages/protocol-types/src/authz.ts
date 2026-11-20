import type { OrganizationId, Timestamp, UserId } from './common';

/** Role assigned to a user inside one organization. */
export type UserRole = 'admin' | 'member' | 'viewer';

/** How the current API caller authenticated. */
export type PrincipalKind = 'legacy_admin' | 'user';

/** One organization. Machines and users belong to an organization. */
export interface Organization {
  id: OrganizationId;
  name: string;
  created_at: Timestamp;
}

/** A user without secrets. Tokens are only returned when created or rotated. */
export interface User {
  id: UserId;
  org_id: OrganizationId;
  name: string;
  role: UserRole;
  created_at: Timestamp;
  updated_at: Timestamp;
  /** When the current bearer token was issued; absent for legacy rows. */
  token_issued_at?: Timestamp | null;
  /** When the current bearer token expires; `null` means never. */
  token_expires_at?: Timestamp | null;
}

/** User plus the generated bearer token. The token is shown exactly once. */
export interface CreatedUser {
  user: User;
  token: string;
}

/** Result of rotating a user token. The old token stops working immediately. */
export interface RotatedUserToken {
  user_id: UserId;
  token: string;
  /** New expiry; `null` means the token never expires. */
  token_expires_at?: Timestamp | null;
}

/** Identity of the current API caller. */
export interface Me {
  kind: PrincipalKind;
  user?: User | null;
  org_id?: OrganizationId | null;
  role: UserRole;
}

/** Create a user inside an existing organization. */
export interface CreateUserRequest {
  org_id: OrganizationId;
  name: string;
  role: UserRole;
  /** Token lifetime override in days; `0` means never, omitted uses the default. */
  token_expires_in_days?: number | null;
}

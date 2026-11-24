import type { PrincipalKind, UserRole } from '@agent-fleet/protocol-types';

/** Shared page frame matching the Machines index layout. */
export const usersPageContainer =
  'mx-auto flex w-full max-w-[1440px] flex-col gap-5 px-5 pb-10 pt-4 sm:px-8 lg:px-10 lg:pb-12';

export const ROLE_LABELS: Record<UserRole, string> = {
  owner: 'Owner',
  admin: 'Admin',
  member: 'Member',
  viewer: 'Viewer',
};

/** Whether the role may manage members and machines. */
export function roleIsAdmin(role?: UserRole | null): boolean {
  return role === 'owner' || role === 'admin';
}

export const ROLE_OPTIONS: Array<{ value: UserRole; label: string }> = [
  { value: 'admin', label: 'Admin' },
  { value: 'member', label: 'Member' },
  { value: 'viewer', label: 'Viewer' },
];

/** Expiry choices shared by the create and rotate dialogs. */
export const EXPIRY_OPTIONS = [
  { value: 'default', label: 'Server default' },
  { value: '7', label: '7 days' },
  { value: '30', label: '30 days' },
  { value: '90', label: '90 days' },
  { value: '365', label: '365 days' },
  { value: 'never', label: 'Never expires' },
];

/** Translate a select value into the API's `token_expires_in_days`. */
export function expiryChoiceToDays(choice: string): number | undefined {
  if (choice === 'default') return undefined;
  if (choice === 'never') return 0;
  const days = Number(choice);
  return Number.isFinite(days) && days >= 0 ? days : undefined;
}

const DAY_MS = 86_400_000;

/** Whole days until expiry, negative when expired, `null` when no expiry. */
export function daysUntilExpiry(expiresAt?: string | null): number | null {
  if (!expiresAt) return null;
  const timestamp = Date.parse(expiresAt);
  if (Number.isNaN(timestamp)) return null;
  return Math.ceil((timestamp - Date.now()) / DAY_MS);
}

/** Compact expiry label used in tables and cards. */
export function tokenExpiryLabel(expiresAt?: string | null): string {
  const days = daysUntilExpiry(expiresAt);
  if (days === null) return 'Never';
  if (days <= 0) return 'Expired';
  if (days === 1) return 'in 1 day';
  return `in ${days} days`;
}

export function tokenIsExpired(expiresAt?: string | null): boolean {
  const days = daysUntilExpiry(expiresAt);
  return days !== null && days <= 0;
}

export function tokenIsExpiringSoon(expiresAt?: string | null): boolean {
  const days = daysUntilExpiry(expiresAt);
  return days !== null && days > 0 && days <= 7;
}

export function principalLabel(kind?: PrincipalKind): string {
  return kind === 'legacy_admin' ? 'Legacy admin' : 'User';
}

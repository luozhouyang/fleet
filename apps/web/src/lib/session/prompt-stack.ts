/**
 * Pure rules for the pending-permission stack above the composer.
 *
 * A session can have several permissions waiting at once. The stack shows one
 * card at a time, keeps the rest queued behind it, and offers a bulk approve
 * for the common case of a batch of reads.
 */

import {
  PermissionStatus,
  type AgentPermissionOption,
  type AgentPermissionRequest,
} from '@agent-fleet/protocol-types';

/** Whether a permission still needs a decision. */
export function permissionIsPending(permission: AgentPermissionRequest): boolean {
  return permission.status === PermissionStatus.Pending;
}

/**
 * The pending permissions in arrival order.
 *
 * The oldest comes first: the agent is blocked on it, so answering it is what
 * unblocks the turn.
 */
export function pendingPermissions(
  permissions: Record<string, AgentPermissionRequest>,
  order: readonly string[],
): AgentPermissionRequest[] {
  const pending: AgentPermissionRequest[] = [];
  for (const id of order) {
    const permission = permissions[id];
    if (permission && permissionIsPending(permission)) pending.push(permission);
  }
  // A permission created by an event that arrived out of order still belongs
  // in the stack; append anything the order array has not seen yet.
  for (const permission of Object.values(permissions)) {
    if (!permissionIsPending(permission)) continue;
    if (pending.some((known) => known.id === permission.id)) continue;
    pending.push(permission);
  }
  return pending.sort((left, right) => {
    const delta = Date.parse(left.created_at) - Date.parse(right.created_at);
    return delta !== 0 ? delta : left.id.localeCompare(right.id);
  });
}

/** Remaining time before `expires_at`, clamped at zero; `null` without one. */
export function remainingSeconds(expiresAt: string | undefined, now: number): number | null {
  if (!expiresAt) return null;
  const expires = Date.parse(expiresAt);
  if (Number.isNaN(expires)) return null;
  return Math.max(0, Math.ceil((expires - now) / 1_000));
}

/** Whether a permission can be decided automatically as "approve". */
export function isApprovable(permission: AgentPermissionRequest): boolean {
  return resolveDecision(permission.options, 'approve') !== null;
}

/**
 * Resolve the option id and direction for a decision.
 *
 * Provider option kinds are `allow_once`, `allow_always`, `reject_once` and
 * `reject_always`. A one-time decision is deliberately preferred over a
 * persistent one: the default keypress must not grant a standing permission
 * the user has not seen in the review dialog.
 */
export function resolveDecision(
  options: AgentPermissionOption[] | undefined,
  decision: 'approve' | 'reject',
): { optionId?: string; approve: boolean } | null {
  const prefix = decision === 'approve' ? 'allow' : 'reject';
  const candidates = (options ?? []).filter((option) => option.kind.startsWith(prefix));
  if (candidates.length > 0) {
    const once = candidates.find((option) => option.kind.endsWith('_once'));
    const match = once ?? candidates[0];
    return { optionId: match.option_id, approve: decision === 'approve' };
  }
  // Without explicit options the backend applies its own default decision.
  if (!options || options.length === 0) {
    return { approve: decision === 'approve' };
  }
  return null;
}

const READ_ONLY_TOOLS = [
  'read',
  'view',
  'cat',
  'open',
  'grep',
  'search',
  'find',
  'rg',
  'glob',
  'list',
  'ls',
  'tree',
];

/**
 * Whether a permission concerns only reading.
 *
 * Bulk approve never covers a command, a write or a delete: those stay
 * individually reviewed, which is the fail-closed default for anything that
 * can change the machine.
 */
export function isReadOnlyPermission(permission: AgentPermissionRequest): boolean {
  if (!isApprovable(permission)) return false;
  const name = permission.tool_name.toLowerCase();
  if (['bash', 'shell', 'exec', 'run', 'command', 'terminal'].some((part) => name.includes(part))) {
    return false;
  }
  return READ_ONLY_TOOLS.some((part) => name.includes(part));
}

/**
 * Whether the bulk approve action is meaningful.
 *
 * Offered only when every pending permission is a read with a clear allow
 * option; one command or write in the stack forces individual review.
 */
export function canBulkApprove(pending: AgentPermissionRequest[]): boolean {
  return pending.length > 1 && pending.every(isReadOnlyPermission);
}

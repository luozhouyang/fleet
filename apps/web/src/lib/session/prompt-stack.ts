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

/**
 * Whole words in a tool name that mark it as a pure read.
 *
 * Matching is per word, never a substring of the compacted name: a tool called
 * `find_and_replace` or `list_and_delete` must not qualify merely because it
 * contains `find` or `list`. `open` is deliberately absent — `open_application`
 * is not a read.
 */
const READ_ONLY_WORDS = new Set([
  'cat',
  'find',
  'glob',
  'grep',
  'list',
  'lookup',
  'ls',
  'read',
  'rg',
  'search',
  'tree',
  'view',
]);

/** Compound names providers use without separators. */
const READ_ONLY_NAMES = new Set([
  'catfile',
  'findfiles',
  'grepsearch',
  'listdir',
  'listfiles',
  'opendir',
  'openfile',
  'readfile',
  'readfiles',
  'readmultiplefiles',
  'readtextfile',
  'searchfiles',
  'viewfile',
]);

/**
 * Name fragments that always mean the tool can change the machine.
 *
 * Substring matching is safe in this direction because the bias is to refuse:
 * a false positive only hides the bulk-approve button.
 */
const MUTATING_TOOL_PARTS = [
  'apply',
  'bash',
  'command',
  'create',
  'delete',
  'edit',
  'exec',
  'move',
  'patch',
  'remove',
  'replace',
  'run',
  'shell',
  'terminal',
  'write',
];

/** Lowercase words of a tool name, split on any non-alphanumeric character. */
function toolWords(name: string): string[] {
  return name
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
}

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
  if (MUTATING_TOOL_PARTS.some((part) => name.includes(part))) return false;
  const compact = name.replace(/[^a-z0-9]/g, '');
  if (READ_ONLY_NAMES.has(compact)) return true;
  return toolWords(name).some((word) => READ_ONLY_WORDS.has(word));
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

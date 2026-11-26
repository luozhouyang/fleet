/**
 * Derived values for the session header: usage, pending counts and the
 * activity label.
 */

import type {
  ActivityChangedPayload,
  SessionStatus,
  UsageUpdatedPayload,
} from '@agent-fleet/protocol-types';

/** Format the agent's usage report into a compact header label. */
export function usageLabel(usage: UsageUpdatedPayload | null): string | null {
  if (!usage) return null;
  const parts: string[] = [];
  if (typeof usage.used === 'number' && typeof usage.size === 'number' && usage.size > 0) {
    const percent = Math.round((usage.used / usage.size) * 100);
    parts.push(`${percent}% context`);
  } else if (typeof usage.used === 'number') {
    parts.push(`${usage.used} used`);
  }
  if (typeof usage.cost === 'number' && usage.cost > 0) {
    parts.push(`$${usage.cost.toFixed(2)}`);
  }
  return parts.length ? parts.join(' · ') : null;
}

export type ActivityTone = 'running' | 'waiting' | 'idle';

/** Map an activity report onto a display tone. */
export function activityTone(
  activity: ActivityChangedPayload | null,
  status: SessionStatus | undefined,
): ActivityTone | null {
  if (!activity) return null;
  switch (activity.state) {
    case 'running':
      return 'running';
    case 'requires_action':
      return 'waiting';
    case 'idle':
      return 'idle';
    default:
      return status === 'running' ? 'running' : null;
  }
}

/** Human label for the header activity chip. */
export function activityLabel(
  activity: ActivityChangedPayload | null,
  status: SessionStatus | undefined,
): string | null {
  if (!activity) return null;
  if (activity.background?.length) {
    return `Background: ${activity.background.join(', ')}`;
  }
  if (activity.agent_initiated && activity.state === 'running') {
    return 'Working autonomously';
  }
  switch (activity.state) {
    case 'running':
      return 'Working';
    case 'requires_action':
      return 'Needs you';
    case 'idle':
      return 'Idle';
    default:
      return status === 'running' ? 'Working' : null;
  }
}

/** Truncate a branch name for the header chip. */
export function branchLabel(branch: string | undefined, max = 28): string | null {
  if (!branch?.trim()) return null;
  const name = branch.trim();
  return name.length > max ? `${name.slice(0, max - 1)}…` : name;
}

/**
 * The number the rail and header show as "pending".
 *
 * Permissions and questions both block the agent, so the count is their sum.
 */
export function pendingPromptCount(permissions: number, questions: number): number {
  return permissions + questions;
}

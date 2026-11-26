import {
  PermissionStatus,
  type AgentPermissionOption,
  type AgentPermissionRequest,
} from '@agent-fleet/protocol-types';
import { describe, expect, it } from 'vitest';
import {
  canBulkApprove,
  isReadOnlyPermission,
  pendingPermissions,
  remainingSeconds,
  resolveDecision,
} from './prompt-stack';

const OPTIONS: AgentPermissionOption[] = [
  { option_id: 'always', name: 'Always allow', kind: 'allow_always' },
  { option_id: 'once', name: 'Allow once', kind: 'allow_once' },
  { option_id: 'deny', name: 'Reject', kind: 'reject_once' },
];

function permission(
  id: string,
  toolName: string,
  createdAt: string,
  status = PermissionStatus.Pending,
  options: AgentPermissionOption[] | undefined = OPTIONS,
): AgentPermissionRequest {
  return {
    id,
    session_id: 's_1',
    tool_name: toolName,
    description: `${toolName} request`,
    input: {},
    options,
    status,
    created_at: createdAt,
  };
}

describe('pending stack', () => {
  it('lists pending permissions oldest first, ignoring resolved ones', () => {
    const permissions = {
      p2: permission('p2', 'Read', '2026-01-01T10:00:05Z'),
      p1: permission('p1', 'Bash', '2026-01-01T10:00:00Z'),
      p3: permission('p3', 'Edit', '2026-01-01T10:00:09Z', PermissionStatus.Approved),
    };
    const pending = pendingPermissions(permissions, ['p3', 'p1']);
    expect(pending.map((item) => item.id)).toEqual(['p1', 'p2']);
  });

  it('counts down to the expiry and clamps at zero', () => {
    const now = Date.parse('2026-01-01T10:00:00Z');
    expect(remainingSeconds('2026-01-01T10:00:30Z', now)).toBe(30);
    expect(remainingSeconds('2026-01-01T09:59:00Z', now)).toBe(0);
    expect(remainingSeconds(undefined, now)).toBeNull();
    expect(remainingSeconds('not-a-date', now)).toBeNull();
  });
});

describe('decisions', () => {
  it('prefers the one-time option over always', () => {
    expect(resolveDecision(OPTIONS, 'approve')).toEqual({
      optionId: 'always',
      approve: true,
    });
    expect(resolveDecision(OPTIONS, 'reject')).toEqual({
      optionId: 'deny',
      approve: false,
    });
  });

  it('falls back to the backend default without options', () => {
    expect(resolveDecision(undefined, 'approve')).toEqual({ approve: true });
    expect(resolveDecision([], 'reject')).toEqual({ approve: false });
  });

  it('reports no decision when every option is for the other direction', () => {
    const rejectsOnly = OPTIONS.filter((option) => option.kind.startsWith('reject'));
    expect(resolveDecision(rejectsOnly, 'approve')).toBeNull();
  });
});

describe('bulk approve', () => {
  it('refuses anything that runs a command', () => {
    expect(isReadOnlyPermission(permission('p1', 'Bash', '2026-01-01T10:00:00Z'))).toBe(false);
    expect(isReadOnlyPermission(permission('p2', 'Read', '2026-01-01T10:00:00Z'))).toBe(true);
    expect(isReadOnlyPermission(permission('p3', 'grep', '2026-01-01T10:00:00Z'))).toBe(true);
  });

  it('requires more than one read and a clear allow option', () => {
    const reads = [
      permission('p1', 'Read', '2026-01-01T10:00:00Z'),
      permission('p2', 'Search', '2026-01-01T10:00:01Z'),
    ];
    expect(canBulkApprove(reads)).toBe(true);
    expect(canBulkApprove([reads[0]])).toBe(false);
    expect(canBulkApprove([...reads, permission('p3', 'Bash', '2026-01-01T10:00:02Z')])).toBe(
      false,
    );
  });
});

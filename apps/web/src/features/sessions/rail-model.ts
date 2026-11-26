/**
 * Pure model for the session rail.
 *
 * The rail renders a flat list of headers and rows so it can be virtualized.
 * Everything here is a pure function of the sessions, the group mode and the
 * attention latch, which keeps the ordering rules testable and the component
 * free of sorting logic.
 */

import { SessionStatus, type AgentSession } from '@agent-fleet/protocol-types';

/** How the rail groups rows. */
export type RailGroupMode = 'none' | 'machine' | 'workspace' | 'status';

/** Rail filter chips. */
export type RailFilter = 'all' | 'running' | 'waiting' | 'failed' | 'closed' | 'archived';

export const RAIL_FILTERS: Array<{ id: RailFilter; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'waiting', label: 'Waiting' },
  { id: 'failed', label: 'Failed' },
  { id: 'closed', label: 'Closed' },
  { id: 'archived', label: 'Archived' },
];

export const GROUP_MODES: Array<{ id: RailGroupMode; label: string }> = [
  { id: 'none', label: 'Ungrouped' },
  { id: 'machine', label: 'Machine' },
  { id: 'workspace', label: 'Workspace' },
  { id: 'status', label: 'Status' },
];

/**
 * Attention level of one session, highest first. Mirrors Orca's status lane:
 * "needs you" outranks a failure, which outranks work in progress.
 */
export type AttentionLevel = 'needs-you' | 'failed' | 'working' | 'idle';

export function attentionLevel(session: AgentSession): AttentionLevel {
  switch (session.status) {
    case SessionStatus.WaitingForPermission:
    case SessionStatus.WaitingForQuestion:
      return 'needs-you';
    case SessionStatus.Failed:
      return 'failed';
    case SessionStatus.Running:
    case SessionStatus.Starting:
    case SessionStatus.Creating:
      return 'working';
    default:
      return 'idle';
  }
}

/** Sort weight for smart sort; lower sorts first. */
const ATTENTION_RANK: Record<AttentionLevel, number> = {
  'needs-you': 0,
  failed: 1,
  working: 2,
  idle: 3,
};

function updatedAt(session: AgentSession): number {
  const parsed = Date.parse(session.updated_at);
  return Number.isNaN(parsed) ? 0 : parsed;
}

/** Whether the chip keeps a session. */
export function matchesFilter(session: AgentSession, filter: RailFilter): boolean {
  switch (filter) {
    case 'running':
      return session.status === SessionStatus.Running || session.status === SessionStatus.Starting;
    case 'waiting':
      return (
        session.status === SessionStatus.WaitingForInput ||
        session.status === SessionStatus.WaitingForPermission ||
        session.status === SessionStatus.WaitingForQuestion
      );
    case 'failed':
      return session.status === SessionStatus.Failed;
    case 'closed':
      return session.status === SessionStatus.Terminated;
    case 'archived':
      return Boolean(session.ui?.archived);
    case 'all':
      // Archived sessions live behind their own chip, not in All.
      return !session.ui?.archived;
  }
}

/** Case-insensitive match over title, ids, provider, cwd, machine and workspace names. */
export function matchesSearch(
  session: AgentSession,
  query: string,
  names: { machine?: string; workspace?: string },
): boolean {
  const term = query.trim().toLowerCase();
  if (!term) return true;
  const haystack = [
    session.title ?? '',
    session.id,
    session.provider,
    session.agent_type,
    session.cwd,
    names.machine ?? '',
    names.workspace ?? '',
    ...(session.ui?.labels ?? []),
  ]
    .join('\n')
    .toLowerCase();
  return term
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

/** Whether a session carries every selected label. */
export function matchesLabels(session: AgentSession, labels: readonly string[]): boolean {
  if (labels.length === 0) return true;
  const sessionLabels = new Set((session.ui?.labels ?? []).map((label) => label.toLowerCase()));
  return labels.every((label) => sessionLabels.has(label.toLowerCase()));
}

/**
 * Sort by recency, with pinned sessions first.
 *
 * Pinned order uses `pinned_rank` when the caller has one, so a drag-reordered
 * pin list survives a refetch.
 */
export function sortByRecency(sessions: readonly AgentSession[]): AgentSession[] {
  return [...sessions].sort((left, right) => {
    const leftPinned = left.ui?.pinned ? 1 : 0;
    const rightPinned = right.ui?.pinned ? 1 : 0;
    if (leftPinned !== rightPinned) return rightPinned - leftPinned;
    if (leftPinned && rightPinned) {
      const leftRank = left.ui?.pinned_rank ?? Number.MAX_SAFE_INTEGER;
      const rightRank = right.ui?.pinned_rank ?? Number.MAX_SAFE_INTEGER;
      if (leftRank !== rightRank) return leftRank - rightRank;
    }
    return updatedAt(right) - updatedAt(left) || left.id.localeCompare(right.id);
  });
}

/**
 * Sort by attention, then recency.
 *
 * `latched` makes the smart order sticky: while any session is working the
 * order follows attention, and once the user has seen it, it stays put rather
 * than reshuffling under the pointer as sessions settle.
 */
export function sortByAttention(
  sessions: readonly AgentSession[],
  options: { latched: boolean },
): AgentSession[] {
  const ranked = sortByRecency(sessions);
  if (!options.latched) return ranked;
  return ranked.sort((left, right) => {
    const leftPinned = left.ui?.pinned ? 1 : 0;
    const rightPinned = right.ui?.pinned ? 1 : 0;
    if (leftPinned !== rightPinned) return rightPinned - leftPinned;
    const rank = ATTENTION_RANK[attentionLevel(left)] - ATTENTION_RANK[attentionLevel(right)];
    return rank || updatedAt(right) - updatedAt(left) || left.id.localeCompare(right.id);
  });
}

/** Whether any session currently needs the user's attention. */
export function hasAttention(sessions: readonly AgentSession[]): boolean {
  return sessions.some((session) => {
    const level = attentionLevel(session);
    return level === 'needs-you' || level === 'failed';
  });
}

/** A row in the flattened rail. */
export type RailRow =
  | {
      kind: 'header';
      key: string;
      label: string;
      count: number;
      collapsed: boolean;
      /** Human-readable group name, shown in tooltips. */
      tone?: AttentionLevel;
    }
  | { kind: 'session'; key: string; session: AgentSession; groupKey: string };

/** A group before flattening. */
export interface RailGroup {
  key: string;
  label: string;
  sessions: AgentSession[];
  /** Count of sessions needing attention; shown on a collapsed header. */
  attention: number;
}

function groupKeyFor(
  session: AgentSession,
  mode: RailGroupMode,
  names: { machine?: string; workspace?: string },
): { key: string; label: string } {
  switch (mode) {
    case 'machine':
      return {
        key: `machine:${session.machine_id}`,
        label: names.machine || 'Unknown machine',
      };
    case 'workspace':
      return session.workspace_id
        ? {
            key: `workspace:${session.workspace_id}`,
            label: names.workspace || 'Workspace',
          }
        : { key: 'workspace:none', label: 'No workspace' };
    case 'status':
      switch (attentionLevel(session)) {
        case 'needs-you':
          return { key: 'status:needs-you', label: 'Needs you' };
        case 'failed':
          return { key: 'status:failed', label: 'Failed' };
        case 'working':
          return { key: 'status:working', label: 'Working' };
        default:
          return { key: 'status:idle', label: 'Idle' };
      }
    case 'none':
      return { key: 'all', label: 'All sessions' };
  }
}

/** Group status order for the `status` mode; groups sort by urgency. */
const STATUS_ORDER = ['status:needs-you', 'status:failed', 'status:working', 'status:idle'];

/**
 * Group the rails' sessions.
 *
 * Pinned sessions form their own group at the top in every mode, so a pin is
 * always one glance away.
 */
export function groupSessions(
  sessions: readonly AgentSession[],
  mode: RailGroupMode,
  names: Map<string, { machine?: string; workspace?: string }>,
): RailGroup[] {
  const groups = new Map<string, RailGroup>();
  const pinned: RailGroup = { key: 'pinned', label: 'Pinned', sessions: [], attention: 0 };

  for (const session of sessions) {
    if (session.ui?.pinned) {
      pinned.sessions.push(session);
      continue;
    }
    const sessionNames = names.get(session.id) ?? {};
    const { key, label } = groupKeyFor(session, mode, sessionNames);
    let group = groups.get(key);
    if (!group) {
      group = { key, label, sessions: [], attention: 0 };
      groups.set(key, group);
    }
    group.sessions.push(session);
  }

  const ordered = [...groups.values()].sort((left, right) => {
    if (mode === 'status') {
      const leftIndex = STATUS_ORDER.indexOf(left.key);
      const rightIndex = STATUS_ORDER.indexOf(right.key);
      if (leftIndex !== rightIndex) return leftIndex - rightIndex;
    }
    return left.label.localeCompare(right.label, undefined, { sensitivity: 'base' });
  });
  for (const group of groups.values()) {
    group.attention = group.sessions.filter((session) => {
      const level = attentionLevel(session);
      return level === 'needs-you' || level === 'failed';
    }).length;
    group.sessions = sortByRecency(group.sessions);
  }

  if (pinned.sessions.length > 0) {
    pinned.attention = pinned.sessions.filter((session) => {
      const level = attentionLevel(session);
      return level === 'needs-you' || level === 'failed';
    }).length;
    pinned.sessions = sortByRecency(pinned.sessions);
    return [pinned, ...ordered];
  }
  return ordered;
}

/** Build the flattened, virtualizable row list. */
export function buildRailRows(
  groups: readonly RailGroup[],
  collapsed: ReadonlySet<string>,
  groupMode: RailGroupMode,
): RailRow[] {
  const rows: RailRow[] = [];
  const showHeaders = groupMode !== 'none' || groups.some((group) => group.key === 'pinned');
  for (const group of groups) {
    const isCollapsed = collapsed.has(group.key);
    if (showHeaders) {
      rows.push({
        kind: 'header',
        key: `header:${group.key}`,
        label: group.label,
        count: group.sessions.length,
        collapsed: isCollapsed,
      });
    }
    if (isCollapsed) continue;
    for (const session of group.sessions) {
      rows.push({
        kind: 'session',
        key: `session:${session.id}`,
        session,
        groupKey: group.key,
      });
    }
  }
  return rows;
}

/** Time bucket of a session, used when the rail is ungrouped. */
export function timeBucket(session: AgentSession, now = Date.now()): 'today' | 'week' | 'older' {
  const updated = updatedAt(session);
  const day = 24 * 60 * 60 * 1000;
  if (now - updated < day) return 'today';
  if (now - updated < 7 * day) return 'week';
  return 'older';
}

/** Human label for a time bucket. */
export function bucketLabel(bucket: 'today' | 'week' | 'older'): string {
  return bucket === 'today' ? 'Today' : bucket === 'week' ? 'Previous 7 days' : 'Older';
}

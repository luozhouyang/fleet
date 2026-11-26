import { SessionStatus, type AgentSession, type SessionUiState } from '@agent-fleet/protocol-types';
import { describe, expect, it } from 'vitest';
import {
  attentionLevel,
  buildRailRows,
  groupSessions,
  hasAttention,
  matchesFilter,
  matchesLabels,
  matchesSearch,
  sortByAttention,
  sortByRecency,
  timeBucket,
} from './rail-model';

function session(
  id: string,
  status: SessionStatus,
  minutesAgo: number,
  ui: Partial<SessionUiState> = {},
): AgentSession {
  const at = new Date(Date.now() - minutesAgo * 60_000).toISOString();
  return {
    id,
    machine_id: 'm_1',
    provider: 'claude',
    agent_type: 'claude-code',
    status,
    cwd: '/repo',
    title: id.toUpperCase(),
    created_at: at,
    updated_at: at,
    ui: {
      pinned: false,
      archived: false,
      unread: false,
      ...ui,
    },
  };
}

describe('rail model', () => {
  it('classifies attention', () => {
    expect(attentionLevel(session('a', SessionStatus.WaitingForPermission, 1))).toBe('needs-you');
    expect(attentionLevel(session('a', SessionStatus.WaitingForQuestion, 1))).toBe('needs-you');
    expect(attentionLevel(session('a', SessionStatus.Failed, 1))).toBe('failed');
    expect(attentionLevel(session('a', SessionStatus.Running, 1))).toBe('working');
    expect(attentionLevel(session('a', SessionStatus.Completed, 1))).toBe('idle');
    expect(hasAttention([session('a', SessionStatus.Failed, 1)])).toBe(true);
    expect(hasAttention([session('a', SessionStatus.Running, 1)])).toBe(false);
  });

  it('filters chips without mixing archived into all', () => {
    const archived = session('z', SessionStatus.Completed, 1, { archived: true });
    const active = session('a', SessionStatus.Running, 2);
    expect(matchesFilter(archived, 'all')).toBe(false);
    expect(matchesFilter(archived, 'archived')).toBe(true);
    expect(matchesFilter(active, 'all')).toBe(true);
    expect(matchesFilter(session('b', SessionStatus.Failed, 1), 'failed')).toBe(true);
    expect(matchesFilter(session('c', SessionStatus.WaitingForPermission, 1), 'waiting')).toBe(
      true,
    );
  });

  it('searches every field and label', () => {
    const target = session('abc', SessionStatus.Running, 1, { labels: ['Backend'] });
    expect(matchesSearch(target, 'claude-code', {})).toBe(true);
    expect(matchesSearch(target, '/repo', {})).toBe(true);
    expect(matchesSearch(target, 'backend', {})).toBe(true);
    expect(matchesSearch(target, 'machine one', { machine: 'Machine One' })).toBe(true);
    expect(matchesSearch(target, 'nope', {})).toBe(false);
    expect(matchesSearch(target, '   ', {})).toBe(true);
  });

  it('requires every selected label', () => {
    const target = session('a', SessionStatus.Running, 1, { labels: ['backend', 'review'] });
    expect(matchesLabels(target, ['backend'])).toBe(true);
    expect(matchesLabels(target, ['backend', 'review'])).toBe(true);
    expect(matchesLabels(target, ['backend', 'ui'])).toBe(false);
    expect(matchesLabels(target, [])).toBe(true);
  });

  it('sorts pinned first by rank, then recency', () => {
    const sessions = [
      session('recent', SessionStatus.Completed, 1),
      session('pinned-b', SessionStatus.Completed, 30, { pinned: true, pinned_rank: 2 }),
      session('pinned-a', SessionStatus.Completed, 40, { pinned: true, pinned_rank: 1 }),
    ];
    const sorted = sortByRecency(sessions).map((item) => item.id);
    expect(sorted).toEqual(['pinned-a', 'pinned-b', 'recent']);
  });

  it('keeps smart order stable when not latched', () => {
    const sessions = [
      session('recent', SessionStatus.Completed, 1),
      session('waiting', SessionStatus.WaitingForPermission, 30),
    ];
    expect(sortByAttention(sessions, { latched: false }).map((item) => item.id)).toEqual([
      'recent',
      'waiting',
    ]);
    expect(sortByAttention(sessions, { latched: true }).map((item) => item.id)).toEqual([
      'waiting',
      'recent',
    ]);
  });

  it('groups pinned sessions into their own top group', () => {
    const sessions = [
      session('pinned', SessionStatus.Completed, 60, { pinned: true }),
      session('a', SessionStatus.Running, 1),
      session('b', SessionStatus.WaitingForPermission, 2),
    ];
    const groups = groupSessions(sessions, 'status', new Map());
    expect(groups[0].key).toBe('pinned');
    expect(groups.map((group) => group.label)).toEqual(['Pinned', 'Needs you', 'Working']);
    expect(groups[1].attention).toBe(1);
  });

  it('collapses groups into fewer rows', () => {
    const sessions = [
      session('a', SessionStatus.Running, 1),
      session('b', SessionStatus.Running, 2),
    ];
    const groups = groupSessions(sessions, 'machine', new Map());
    const rows = buildRailRows(groups, new Set([groups[0].key]), 'machine');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ kind: 'header', count: 2, collapsed: true });

    const expanded = buildRailRows(groups, new Set(), 'machine');
    expect(expanded.map((row) => row.kind)).toEqual(['header', 'session', 'session']);
  });

  it('buckets by age', () => {
    expect(timeBucket(session('a', SessionStatus.Running, 60))).toBe('today');
    expect(timeBucket(session('a', SessionStatus.Running, 60 * 24 * 3))).toBe('week');
    expect(timeBucket(session('a', SessionStatus.Running, 60 * 24 * 30))).toBe('older');
  });
});

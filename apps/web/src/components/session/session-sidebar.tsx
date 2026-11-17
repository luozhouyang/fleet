import type { EventConnectionState } from '@agent-fleet/api-client';
import { MachineStatus, SessionStatus, type AgentSession } from '@agent-fleet/protocol-types';
import { Link } from '@tanstack/react-router';
import { ChevronRight, Plus, Search, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { Input } from '@/components/ui/input';
import { StatusDot } from '@/components/ui/status-dot';
import { useEventConnectionState } from '@/lib/events/useEventConnection';
import { useMachines, useSessionList } from '@/lib/query/hooks';
import { sessionIsActive, sessionStatusMeta, type StatusTone } from '@/lib/status';
import { cn } from '@/lib/utils/cn';
import { formatRelativeTime, getErrorMessage, shortId } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';
import { sessionTitle } from './session-utils';

type SessionFilter = 'all' | 'running' | 'waiting' | 'closed';

const FILTERS: Array<{ id: SessionFilter; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'waiting', label: 'Waiting' },
  { id: 'closed', label: 'Closed' },
];

function isRunningStatus(status: SessionStatus): boolean {
  return status === SessionStatus.Running || status === SessionStatus.Starting;
}

function isWaitingStatus(status: SessionStatus): boolean {
  return (
    status === SessionStatus.WaitingForInput ||
    status === SessionStatus.WaitingForPermission ||
    status === SessionStatus.WaitingForQuestion
  );
}

function isClosedStatus(status: SessionStatus): boolean {
  return status === SessionStatus.Terminated;
}

function matchesFilter(session: AgentSession, filter: SessionFilter): boolean {
  if (filter === 'closed') return isClosedStatus(session.status);
  if (filter === 'running') return isRunningStatus(session.status);
  if (filter === 'waiting') return isWaitingStatus(session.status);
  // "All" is the default Active view; archived sessions live behind Closed.
  return !isClosedStatus(session.status);
}

function connectionMeta(state: EventConnectionState): {
  label: string;
  tone: StatusTone;
  live: boolean;
} {
  switch (state) {
    case 'connected':
      return { label: 'Live', tone: 'running', live: true };
    case 'connecting':
      return { label: 'Connecting', tone: 'waiting', live: false };
    case 'reconnecting':
      return { label: 'Reconnecting', tone: 'waiting', live: false };
    case 'failed':
      return { label: 'Connection failed', tone: 'failed', live: false };
    default:
      return { label: 'Offline', tone: 'offline', live: false };
  }
}

interface SessionGroup {
  key: string;
  machineName: string;
  workspaceName: string;
  sessions: AgentSession[];
}

export function SessionSidebar({
  selectedId,
  mobileOpen,
  onCloseMobile,
  onNewSession,
}: {
  selectedId: string;
  mobileOpen: boolean;
  onCloseMobile: () => void;
  onNewSession: () => void;
}) {
  const sessionsQuery = useSessionList(true);
  const machinesQuery = useMachines();
  const connection = useEventConnectionState();
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState<SessionFilter>('all');
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!mobileOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !event.defaultPrevented) onCloseMobile();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [mobileOpen, onCloseMobile]);

  const toggleGroup = (key: string) => {
    setCollapsedGroups((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const sessions = useMemo(
    () =>
      [...(sessionsQuery.data ?? [])].sort(
        (left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at),
      ),
    [sessionsQuery.data],
  );

  const machineNames = useMemo(
    () =>
      new Map(
        (machinesQuery.data ?? []).map(
          (machine) => [machine.id, machineDisplayName(machine)] as const,
        ),
      ),
    [machinesQuery.data],
  );

  const workspaceNames = useMemo(
    () =>
      new Map(
        (machinesQuery.data ?? []).flatMap((machine) =>
          (machine.workspaces ?? []).map((workspace) => [workspace.id, workspace.name] as const),
        ),
      ),
    [machinesQuery.data],
  );

  const counts = useMemo(
    () => ({
      all: sessions.filter((session) => !isClosedStatus(session.status)).length,
      running: sessions.filter((session) => isRunningStatus(session.status)).length,
      waiting: sessions.filter((session) => isWaitingStatus(session.status)).length,
      closed: sessions.filter((session) => isClosedStatus(session.status)).length,
    }),
    [sessions],
  );

  const activeCount = useMemo(
    () => sessions.filter((session) => sessionIsActive(session.status)).length,
    [sessions],
  );

  const groups = useMemo<SessionGroup[]>(() => {
    const term = search.trim().toLowerCase();
    const filtered = sessions.filter((session) => {
      if (!matchesFilter(session, filter)) return false;
      if (!term) return true;
      const haystack = [
        sessionTitle(session),
        machineNames.get(session.machine_id) ?? '',
        session.workspace_id ? (workspaceNames.get(session.workspace_id) ?? '') : '',
      ]
        .join(' ')
        .toLowerCase();
      return haystack.includes(term);
    });

    const byKey = new Map<string, SessionGroup>();
    for (const session of filtered) {
      const key = `${session.machine_id}::${session.workspace_id ?? ''}`;
      let group = byKey.get(key);
      if (!group) {
        group = {
          key,
          machineName: machineNames.get(session.machine_id) ?? 'Unknown machine',
          workspaceName: session.workspace_id
            ? (workspaceNames.get(session.workspace_id) ?? shortId(session.workspace_id, 12))
            : 'No workspace',
          sessions: [],
        };
        byKey.set(key, group);
      }
      group.sessions.push(session);
    }

    return [...byKey.values()].sort(
      (left, right) =>
        left.machineName.localeCompare(right.machineName, undefined, {
          sensitivity: 'base',
        }) ||
        left.workspaceName.localeCompare(right.workspaceName, undefined, {
          sensitivity: 'base',
        }),
    );
  }, [sessions, filter, search, machineNames, workspaceNames]);

  const machines = machinesQuery.data ?? [];
  const onlineCount = machines.filter(
    (machine) =>
      machine.status === MachineStatus.Online || machine.status === MachineStatus.Degraded,
  ).length;
  const connectionState = connectionMeta(connection);

  return (
    <aside
      aria-label="Sessions"
      role={mobileOpen ? 'dialog' : undefined}
      aria-modal={mobileOpen ? true : undefined}
      className={cn(
        'min-h-0 shrink-0 flex-col border-r border-border bg-panel',
        mobileOpen
          ? 'fixed bottom-0 left-0 top-14 z-50 flex w-[320px] max-w-[85vw] shadow-modal md:static md:bottom-auto md:top-auto md:z-auto md:w-[280px] md:max-w-none md:shadow-none xl:w-[320px]'
          : 'hidden md:flex md:w-[280px] xl:w-[320px]',
      )}
    >
      <div className="flex h-full min-h-0 flex-col">
        <div className="shrink-0 px-3 pt-3">
          <div className="flex items-center justify-between gap-2">
            <div className="flex min-w-0 items-baseline gap-2">
              <h2 className="font-display text-[18px] font-bold leading-6 tracking-[-0.01em] text-foreground">
                Sessions
              </h2>
              <span className="font-mono text-[9px] uppercase tracking-[0.1em] text-faint">
                {activeCount} active
              </span>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <Button
                variant="ghost"
                size="icon-sm"
                className="md:hidden"
                aria-label="Close session list"
                onClick={onCloseMobile}
              >
                <X className="size-4" />
              </Button>
              <Button
                variant="secondary"
                size="icon-sm"
                aria-label="New session"
                title="New session"
                onClick={onNewSession}
              >
                <Plus className="size-4" />
              </Button>
            </div>
          </div>

          <div className="relative mt-3">
            <Search
              className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint"
              aria-hidden="true"
            />
            <Input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Search sessions"
              aria-label="Search sessions"
              className="pl-8"
              type="search"
            />
          </div>

          <div className="mt-2 flex items-center gap-3 pb-1">
            {FILTERS.map((item) => {
              const active = filter === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  aria-pressed={active}
                  onClick={() => setFilter(item.id)}
                  className={cn(
                    '-my-1 rounded-sm px-1 py-1 font-mono text-[9px] uppercase tracking-[0.1em] transition-colors',
                    active ? 'text-foreground' : 'text-faint hover:text-muted-foreground',
                  )}
                >
                  {item.label}
                  <span className={cn('ml-1', active ? 'text-primary-hover' : 'text-faint')}>
                    {counts[item.id]}
                  </span>
                </button>
              );
            })}
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
          {sessionsQuery.isLoading ? (
            <LoadingState label="Loading sessions…" className="py-10" />
          ) : sessionsQuery.isError ? (
            <ErrorState
              title="Could not load sessions"
              message={getErrorMessage(sessionsQuery.error)}
              onRetry={() => void sessionsQuery.refetch()}
              className="py-10"
            />
          ) : groups.length === 0 ? (
            <EmptyState
              title={sessions.length === 0 ? 'No sessions yet' : 'No matching sessions'}
              description={
                sessions.length === 0
                  ? 'Start a session to see the agent run here.'
                  : 'Adjust the search or filter to find a session.'
              }
              className="py-10"
            />
          ) : (
            groups.map((group) => {
              const collapsed = collapsedGroups.has(group.key);
              return (
                <section key={group.key} className="pt-3 first:pt-1">
                  <div className="flex items-center justify-between gap-2 px-1.5 pb-1">
                    <button
                      type="button"
                      onClick={() => toggleGroup(group.key)}
                      aria-expanded={!collapsed}
                      className="flex min-w-0 items-center gap-1 rounded-sm px-1 py-0.5 text-left"
                    >
                      <ChevronRight
                        className={cn(
                          'size-2.5 shrink-0 text-faint transition-transform',
                          !collapsed && 'rotate-90',
                        )}
                        aria-hidden="true"
                      />
                      <span
                        className="truncate font-mono text-[9px] uppercase tracking-[0.12em] text-faint"
                        title={`${group.machineName} / ${group.workspaceName}`}
                      >
                        {group.machineName} / {group.workspaceName}
                      </span>
                    </button>
                    <span className="shrink-0 font-mono text-[9px] text-faint">
                      {group.sessions.length}
                    </span>
                  </div>
                  {collapsed ? null : (
                    <div className="flex flex-col gap-0.5">
                      {group.sessions.map((session) => (
                        <SessionRow
                          key={session.id}
                          session={session}
                          selected={session.id === selectedId}
                          onNavigate={onCloseMobile}
                        />
                      ))}
                    </div>
                  )}
                </section>
              );
            })
          )}
        </div>

        <div className="shrink-0 border-t border-border px-3 py-2.5">
          <div className="flex items-center justify-between gap-2">
            <StatusDot
              tone={connectionState.tone}
              size="sm"
              label={connectionState.label}
              pulse={connectionState.live}
            />
            <span className="truncate font-mono text-[9px] text-faint">
              {machines.length} machines · {onlineCount} online
            </span>
          </div>
        </div>
      </div>
    </aside>
  );
}

function SessionRow({
  session,
  selected,
  onNavigate,
}: {
  session: AgentSession;
  selected: boolean;
  onNavigate: () => void;
}) {
  const status = sessionStatusMeta(session.status);
  return (
    <Link
      to="/sessions/$sessionId"
      params={{ sessionId: session.id }}
      onClick={onNavigate}
      aria-current={selected ? 'page' : undefined}
      data-testid="session-row"
      data-id={session.id}
      className={cn(
        'flex h-[50px] flex-col justify-center rounded-md px-2.5 transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40',
        selected ? 'bg-selected' : 'hover:bg-muted',
      )}
    >
      <span
        className={cn(
          'truncate text-[12.5px] font-bold leading-4',
          selected ? 'text-primary' : 'text-foreground',
        )}
      >
        {sessionTitle(session)}
      </span>
      <span className="mt-0.5 flex min-w-0 items-center gap-1.5">
        <StatusDot tone={status.tone} size="sm" label={status.label} />
        <span className="shrink-0 text-[9px] leading-4 text-faint">·</span>
        <span className="shrink-0 font-mono text-[9px] text-faint">
          {formatRelativeTime(session.updated_at)}
        </span>
      </span>
    </Link>
  );
}

import { MachineStatus, type AgentSession, type Machine } from '@agent-fleet/protocol-types';
import { StatusDot, cn } from '@agent-fleet/ui';
import { useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate, useRouterState } from '@tanstack/react-router';
import { useMemo, useState } from 'react';
import { NewSessionDialog } from '@/components/dialogs/NewSessionDialog';
import {
  isSessionActive,
  sessionLabel,
  sessionStatusTone,
} from '@/components/status/session-status';
import { useMachines, useSessionList } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';

interface CreateTarget {
  machine: Machine;
  workspaceId: string;
}

function machineTone(status: MachineStatus) {
  switch (status) {
    case MachineStatus.Online:
      return 'green' as const;
    case MachineStatus.Degraded:
      return 'amber' as const;
    case MachineStatus.Offline:
      return 'slate' as const;
    default:
      return 'neutral' as const;
  }
}

function byUpdatedDesc(left: AgentSession, right: AgentSession) {
  return Date.parse(right.updated_at) - Date.parse(left.updated_at);
}

export function FleetTree() {
  const machinesQuery = useMachines();
  const sessionsQuery = useSessionList();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [createTarget, setCreateTarget] = useState<CreateTarget | null>(null);

  const machines = useMemo(
    () => [...(machinesQuery.data ?? [])].sort((left, right) => left.name.localeCompare(right.name)),
    [machinesQuery.data],
  );

  const sessionsByWorkspace = useMemo(() => {
    const map = new Map<string, AgentSession[]>();
    for (const session of sessionsQuery.data ?? []) {
      if (!session.workspace_id) continue;
      const list = map.get(session.workspace_id) ?? [];
      list.push(session);
      map.set(session.workspace_id, list);
    }
    for (const list of map.values()) list.sort(byUpdatedDesc);
    return map;
  }, [sessionsQuery.data]);

  const orphanSessionsByMachine = useMemo(() => {
    const map = new Map<string, AgentSession[]>();
    for (const session of sessionsQuery.data ?? []) {
      if (session.workspace_id) continue;
      const list = map.get(session.machine_id) ?? [];
      list.push(session);
      map.set(session.machine_id, list);
    }
    for (const list of map.values()) list.sort(byUpdatedDesc);
    return map;
  }, [sessionsQuery.data]);

  if (machinesQuery.isLoading) {
    return <p className="mt-6 px-3 text-[11px] text-slate-600">Loading fleet…</p>;
  }

  if (machinesQuery.isError) {
    return (
      <div className="mt-6 px-3">
        <p className="text-[11px] text-red-300">Could not load machines.</p>
        <button
          type="button"
          onClick={() => void machinesQuery.refetch()}
          className="mt-1 text-[11px] font-medium text-teal-300 hover:text-teal-200"
        >
          Retry
        </button>
      </div>
    );
  }

  if (machines.length === 0) {
    return <p className="mt-6 px-3 text-[11px] text-slate-600">No machines registered yet.</p>;
  }

  return (
    <div className="mt-6">
      <div className="mb-2 flex items-center justify-between px-3">
        <span className="text-[10px] font-semibold uppercase tracking-[0.18em] text-slate-600">
          Fleet
        </span>
        {sessionsQuery.isFetching ? (
          <span className="size-1.5 rounded-full bg-teal-400/60" aria-hidden="true" />
        ) : null}
      </div>

      <div className="space-y-3">
        {machines.map((machine) => {
          const isCollapsed = collapsed[machine.id] ?? false;
          const workspaces = [...(machine.workspaces ?? [])].sort((left, right) =>
            left.name.localeCompare(right.name),
          );
          const availableProvider = (machine.providers ?? []).some(
            (provider) => provider.available,
          );
          const orphans = orphanSessionsByMachine.get(machine.id) ?? [];

          return (
            <div key={machine.id}>
              <div className="flex items-center gap-1 rounded-md px-1.5 py-1 hover:bg-slate-900/60">
                <button
                  type="button"
                  onClick={() =>
                    setCollapsed((current) => ({
                      ...current,
                      [machine.id]: !isCollapsed,
                    }))
                  }
                  aria-expanded={!isCollapsed}
                  aria-label={`${isCollapsed ? 'Expand' : 'Collapse'} ${machine.name}`}
                  className="flex size-5 shrink-0 items-center justify-center rounded text-slate-500 hover:text-slate-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60"
                >
                  <svg
                    viewBox="0 0 24 24"
                    fill="none"
                    className={cn('size-3 transition-transform', isCollapsed ? '' : 'rotate-90')}
                    aria-hidden="true"
                  >
                    <path
                      d="m9 6 6 6-6 6"
                      stroke="currentColor"
                      strokeWidth="1.8"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                </button>
                <Link
                  to="/machines/$machineId"
                  params={{ machineId: machine.id }}
                  className="flex min-w-0 flex-1 items-center gap-2 rounded px-1 py-0.5 text-xs font-semibold text-slate-300 hover:text-teal-200"
                >
                  <span className="truncate">{machine.name}</span>
                  <StatusDot
                    tone={machineTone(machine.status)}
                    pulse={
                      machine.status === MachineStatus.Online ||
                      machine.status === MachineStatus.Degraded
                    }
                    className="size-1.5"
                  />
                </Link>
              </div>

              {isCollapsed ? null : (
                <div className="ml-3 mt-1 space-y-1.5 border-l border-slate-800/70 pl-2">
                  {workspaces.length === 0 ? (
                    <p className="px-2 text-[11px] text-slate-600">
                      No workspaces. Add one from the machine page.
                    </p>
                  ) : null}

                  {workspaces.map((workspace) => {
                    const workspaceSessions = sessionsByWorkspace.get(workspace.id) ?? [];
                    const workspaceActive = pathname === `/workspaces/${workspace.id}`;

                    return (
                      <div key={workspace.id}>
                        <div
                          className={cn(
                            'group flex items-center gap-1 rounded-md',
                            workspaceActive ? 'bg-slate-900' : 'hover:bg-slate-900/60',
                          )}
                        >
                          <Link
                            to="/workspaces/$workspaceId"
                            params={{ workspaceId: workspace.id }}
                            className={cn(
                              'flex min-w-0 flex-1 items-center gap-2 rounded px-2 py-1.5 text-xs',
                              workspaceActive
                                ? 'font-medium text-teal-200'
                                : 'text-slate-400 hover:text-slate-200',
                            )}
                          >
                            <svg
                              viewBox="0 0 24 24"
                              fill="none"
                              className="size-3.5 shrink-0 text-slate-500"
                              aria-hidden="true"
                            >
                              <path
                                d="M3.5 6.5A1.5 1.5 0 0 1 5 5h4l2 2h8a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5v-11Z"
                                stroke="currentColor"
                                strokeWidth="1.5"
                                strokeLinejoin="round"
                              />
                            </svg>
                            <span className="truncate">{workspace.name}</span>
                          </Link>
                          <button
                            type="button"
                            onClick={() =>
                              setCreateTarget({ machine, workspaceId: workspace.id })
                            }
                            disabled={!availableProvider}
                            aria-label={`New session in ${workspace.name}`}
                            title={
                              availableProvider
                                ? `New session in ${workspace.name}`
                                : 'No available provider on this machine'
                            }
                            className="mr-1 flex size-6 shrink-0 items-center justify-center rounded text-slate-500 opacity-70 transition-colors hover:bg-slate-800 hover:text-teal-300 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60 disabled:cursor-not-allowed disabled:opacity-30"
                          >
                            <svg viewBox="0 0 24 24" fill="none" className="size-3.5" aria-hidden="true">
                              <path
                                d="M12 5v14M5 12h14"
                                stroke="currentColor"
                                strokeWidth="1.8"
                                strokeLinecap="round"
                              />
                            </svg>
                          </button>
                        </div>

                        {workspaceSessions.length ? (
                          <div className="ml-3 mt-0.5 space-y-0.5 border-l border-slate-800/50 pl-2">
                            {workspaceSessions.map((session) => (
                              <SessionLink
                                key={session.id}
                                session={session}
                                active={pathname === `/sessions/${session.id}`}
                              />
                            ))}
                          </div>
                        ) : null}
                      </div>
                    );
                  })}

                  {orphans.length ? (
                    <div>
                      <p className="px-2 py-1 text-[10px] uppercase tracking-[0.14em] text-slate-600">
                        No workspace
                      </p>
                      <div className="space-y-0.5">
                        {orphans.map((session) => (
                          <SessionLink
                            key={session.id}
                            session={session}
                            active={pathname === `/sessions/${session.id}`}
                          />
                        ))}
                      </div>
                    </div>
                  ) : null}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {createTarget ? (
        <NewSessionDialog
          open
          machine={createTarget.machine}
          providers={createTarget.machine.providers ?? []}
          defaultWorkspaceId={createTarget.workspaceId}
          onClose={() => setCreateTarget(null)}
          onCreated={(session) => {
            setCreateTarget(null);
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
            void navigate({
              to: '/sessions/$sessionId',
              params: { sessionId: session.id },
            });
          }}
        />
      ) : null}
    </div>
  );
}

function SessionLink({ session, active }: { session: AgentSession; active: boolean }) {
  return (
    <Link
      to="/sessions/$sessionId"
      params={{ sessionId: session.id }}
      title={session.title || session.id}
      className={cn(
        'flex items-center gap-2 rounded px-2 py-1 text-[11px]',
        active
          ? 'bg-teal-500/10 text-teal-200'
          : 'text-slate-500 hover:bg-slate-900/70 hover:text-slate-300',
      )}
    >
      <StatusDot
        tone={sessionStatusTone(session.status)}
        pulse={isSessionActive(session.status)}
        className="size-1.5 shrink-0"
      />
      <span className="truncate">{sessionLabel(session)}</span>
    </Link>
  );
}

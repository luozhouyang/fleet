import type { AgentSession } from '@agent-fleet/protocol-types';
import { Link, useNavigate } from '@tanstack/react-router';
import { ArrowRight, Plus, QrCode, Server } from 'lucide-react';
import { useMemo, useState } from 'react';
import { AddWorkspaceDialog } from '@/components/dialogs/add-workspace-dialog';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { StatusDot } from '@/components/ui/status-dot';
import { useMe } from '@/features/users/hooks';
import { roleIsAdmin } from '@/features/users/user-utils';
import { useActiveOrgId } from '@/lib/api/organization';
import { useMachines, useRevokedMachines, useSessionList } from '@/lib/query/hooks';
import { useUnrevokeMachine } from '@/lib/query/mutations';
import { machineStatusMeta, sessionIsActive, sessionStatusMeta } from '@/lib/status';
import { formatDateTime, getErrorMessage, shortId } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';
import {
  formatCompactRelativeTime,
  machineIsOnline,
  MachineStatusIndicator,
  PageKicker,
  Panel,
  PanelHeading,
  pageContainer,
} from './machine-ui';
import { PairMachineDialog } from './PairMachineDialog';

export function MachinesPage() {
  const navigate = useNavigate();
  const machinesQuery = useMachines();
  const sessionsQuery = useSessionList();
  const revokedQuery = useRevokedMachines();
  const unrevokeMutation = useUnrevokeMachine();
  const meQuery = useMe();
  const storedOrgId = useActiveOrgId();
  const orgId = storedOrgId || meQuery.data?.org_id || '';
  const canPair = meQuery.data?.kind === 'legacy_admin' || roleIsAdmin(meQuery.data?.role);
  const [addWorkspaceOpen, setAddWorkspaceOpen] = useState(false);
  const [pairOpen, setPairOpen] = useState(false);

  const machines = machinesQuery.data ?? [];
  const sessions = sessionsQuery.data ?? [];
  const revokedMachines = revokedQuery.data ?? [];

  const onlineCount = machines.filter((machine) => machineIsOnline(machine.status)).length;
  const busyWorkspaceCount = new Set(
    sessions
      .filter((session) => sessionIsActive(session.status))
      .map((session) => session.workspace_id)
      .filter((workspaceId): workspaceId is string => Boolean(workspaceId)),
  ).size;

  const sessionsByMachine = useMemo(() => {
    const result = new Map<string, AgentSession[]>();
    for (const session of sessionsQuery.data ?? []) {
      const list = result.get(session.machine_id);
      if (list) {
        list.push(session);
      } else {
        result.set(session.machine_id, [session]);
      }
    }
    return result;
  }, [sessionsQuery.data]);

  const recentSessions = useMemo(
    () =>
      [...(sessionsQuery.data ?? [])]
        .sort((left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at))
        .slice(0, 5),
    [sessionsQuery.data],
  );

  const machinesById = useMemo(
    () => new Map((machinesQuery.data ?? []).map((machine) => [machine.id, machine] as const)),
    [machinesQuery.data],
  );

  const sessionsUnavailable =
    sessionsQuery.isLoading || (sessionsQuery.isError && !sessionsQuery.data);

  const subtitle = machinesQuery.isLoading
    ? 'Loading daemon registry…'
    : machinesQuery.isError && !machinesQuery.data
      ? 'Fleet registry unavailable'
      : `${machines.length} machines · ${onlineCount} online · ${busyWorkspaceCount} workspaces busy`;

  return (
    <div className="h-full overflow-y-auto">
      <div className={pageContainer}>
        <header className="flex flex-wrap items-end justify-between gap-4">
          <div className="flex min-w-0 flex-col gap-1">
            <PageKicker>FLEET / MACHINES</PageKicker>
            <h1 className="font-display text-[30px] font-bold leading-9 tracking-[-0.02em] text-foreground">
              Machines
            </h1>
            <p className="text-[12.5px] leading-5 text-muted-foreground">{subtitle}</p>
          </div>
          <div className="flex items-center gap-2">
            {canPair ? (
              <Button variant="secondary" onClick={() => setPairOpen(true)}>
                <QrCode className="size-3.5" aria-hidden="true" />
                Pair machine
              </Button>
            ) : null}
            <Button
              variant="primary"
              onClick={() => setAddWorkspaceOpen(true)}
              disabled={machines.length === 0}
              title={
                machines.length ? undefined : 'Connect a machine before registering a workspace'
              }
            >
              <Plus className="size-3.5" aria-hidden="true" />
              Add workspace
            </Button>
          </div>
        </header>

        {machinesQuery.isError && !machinesQuery.data ? (
          <Panel>
            <ErrorState
              title="Could not load machines"
              message={getErrorMessage(machinesQuery.error)}
              onRetry={() => void machinesQuery.refetch()}
            />
          </Panel>
        ) : machinesQuery.isLoading ? (
          <Panel>
            <LoadingState label="Loading machines…" />
          </Panel>
        ) : machines.length === 0 ? (
          <Panel>
            <EmptyState
              title="No machines connected"
              description="Start an agent daemon and point it at this admin. The machine registers automatically once it authenticates, then its workspaces and providers appear here."
              icon={<Server className="size-5" aria-hidden="true" />}
              action={
                canPair ? (
                  <Button variant="secondary" size="sm" onClick={() => setPairOpen(true)}>
                    Pair machine
                  </Button>
                ) : (
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => void machinesQuery.refetch()}
                  >
                    Refresh
                  </Button>
                )
              }
            />
          </Panel>
        ) : (
          <Panel>
            <div className="overflow-x-auto">
              <table className="w-full min-w-[900px] border-collapse text-left">
                <thead>
                  <tr className="h-9 bg-muted">
                    <th
                      scope="col"
                      className="w-[30%] px-5 font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
                    >
                      Machine
                    </th>
                    <th
                      scope="col"
                      className="w-[13%] px-5 font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
                    >
                      OS
                    </th>
                    <th
                      scope="col"
                      className="w-[10%] px-5 font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
                    >
                      Daemon
                    </th>
                    <th
                      scope="col"
                      className="w-[12%] px-5 font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
                    >
                      Workspaces
                    </th>
                    <th
                      scope="col"
                      className="w-[12%] px-5 font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
                    >
                      Sessions
                    </th>
                    <th
                      scope="col"
                      className="w-[12%] px-5 font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
                    >
                      Heartbeat
                    </th>
                    <th
                      scope="col"
                      className="w-[11%] px-5 text-right font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
                    >
                      Status
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {machines.map((machine) => {
                    const machineSessions = sessionsByMachine.get(machine.id) ?? [];
                    const activeSessions = machineSessions.filter((session) =>
                      sessionIsActive(session.status),
                    ).length;
                    const workspaceCount = Math.max(
                      machine.workspace_count ?? 0,
                      machine.workspaces?.length ?? 0,
                    );
                    const machineMeta = machineStatusMeta(machine.status);

                    return (
                      <tr
                        key={machine.id}
                        data-testid="machine-row"
                        data-id={machine.id}
                        onClick={() =>
                          void navigate({
                            to: '/machines/$machineId',
                            params: { machineId: machine.id },
                          })
                        }
                        className="h-16 cursor-pointer border-t border-border transition-colors hover:bg-muted"
                      >
                        <td className="px-5 py-2.5">
                          <div className="flex min-w-0 items-center gap-2.5">
                            <StatusDot
                              tone={machineMeta.tone}
                              size="sm"
                              pulse={machineMeta.tone === 'running'}
                            />
                            <div className="min-w-0">
                              <Link
                                to="/machines/$machineId"
                                params={{ machineId: machine.id }}
                                onClick={(event) => event.stopPropagation()}
                                aria-label={`Open machine ${machineDisplayName(machine)}`}
                                className="block truncate text-[13px] font-bold leading-5 text-foreground transition-colors hover:text-primary"
                              >
                                {machineDisplayName(machine)}
                              </Link>
                              <span className="block truncate font-mono text-[9.5px] leading-4 text-faint">
                                {machine.hostname}
                              </span>
                            </div>
                          </div>
                        </td>
                        <td className="px-5 py-2.5">
                          <span className="font-mono text-[11px] text-muted-foreground">
                            {machine.os} / {machine.arch}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <span className="font-mono text-[11px] text-muted-foreground">
                            v{machine.daemon_version}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <span className="text-[13px] font-bold text-foreground">
                            {workspaceCount}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          {sessionsUnavailable ? (
                            <span className="font-mono text-[11px] text-faint">—</span>
                          ) : (
                            <>
                              <span className="block text-[13px] font-bold leading-5 text-foreground">
                                {machineSessions.length}
                              </span>
                              <span className="block font-mono text-[9px] leading-4 text-faint">
                                {activeSessions} active
                              </span>
                            </>
                          )}
                        </td>
                        <td className="px-5 py-2.5">
                          <span
                            className="font-mono text-[11px] text-muted-foreground"
                            title={formatDateTime(machine.last_seen_at)}
                          >
                            {formatCompactRelativeTime(machine.last_seen_at)}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <span className="flex justify-end">
                            <MachineStatusIndicator status={machine.status} />
                          </span>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </Panel>
        )}

        {revokedMachines.length > 0 ? (
          <Panel>
            <PanelHeading
              title="Revoked machines"
              trailing={
                <span className="font-mono text-[9px] text-faint">
                  {revokedMachines.length} revoked
                </span>
              }
            />
            <div className="divide-y divide-border">
              {revokedMachines.map((revoked) => (
                <div
                  key={revoked.machine_id}
                  className="flex items-center justify-between gap-4 px-5 py-3"
                >
                  <div className="min-w-0">
                    <p className="truncate text-[11px] font-bold text-foreground">
                      {revoked.machine_name?.trim() || 'Unknown machine'}
                    </p>
                    <p className="truncate font-mono text-[9px] leading-4 text-faint">
                      revoked {formatDateTime(revoked.revoked_at)}
                      {revoked.reason ? ` · ${revoked.reason}` : ''}
                    </p>
                  </div>
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={unrevokeMutation.isPending}
                    onClick={() => unrevokeMutation.mutate(revoked.machine_id)}
                  >
                    {unrevokeMutation.isPending ? 'Restoring…' : 'Unrevoke'}
                  </Button>
                </div>
              ))}
            </div>
            {unrevokeMutation.error ? (
              <p className="border-t border-border bg-destructive-soft px-5 py-2 text-[11px] text-destructive">
                {getErrorMessage(unrevokeMutation.error)}
              </p>
            ) : null}
          </Panel>
        ) : null}

        {machinesQuery.isSuccess && machines.length > 0 ? (
          <Panel>
            <PanelHeading
              title="Recent sessions"
              trailing={
                <Link
                  to="/sessions"
                  className="inline-flex items-center gap-1 text-[11px] font-bold text-primary transition-colors hover:text-primary-hover"
                >
                  View all
                  <ArrowRight className="size-3" aria-hidden="true" />
                </Link>
              }
            />
            {sessionsQuery.isError ? (
              <ErrorState
                title="Could not load sessions"
                message={getErrorMessage(sessionsQuery.error)}
                onRetry={() => void sessionsQuery.refetch()}
                className="py-10"
              />
            ) : sessionsQuery.isLoading ? (
              <LoadingState label="Loading recent sessions…" className="py-10" />
            ) : recentSessions.length === 0 ? (
              <EmptyState
                title="No sessions yet"
                description="Open a machine with an available provider to start a coding-agent session."
              />
            ) : (
              <div>
                {recentSessions.map((session) => {
                  const machine = machinesById.get(session.machine_id);
                  const status = sessionStatusMeta(session.status);

                  return (
                    <Link
                      key={session.id}
                      to="/sessions/$sessionId"
                      params={{ sessionId: session.id }}
                      className="grid grid-cols-[minmax(0,2.2fr)_minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_90px] items-center gap-4 border-t border-border px-5 py-3.5 transition-colors first:border-t-0 hover:bg-muted"
                    >
                      <span className="truncate text-[13px] font-bold text-foreground">
                        {session.title || `Session ${shortId(session.id, 8)}`}
                      </span>
                      <span className="flex min-w-0 items-center gap-1.5">
                        {machine ? (
                          <StatusDot
                            tone={machineStatusMeta(machine.status).tone}
                            size="xs"
                            label={machineStatusMeta(machine.status).label}
                            labelClassName="sr-only"
                          />
                        ) : null}
                        <span className="truncate text-[11.5px] text-muted-foreground">
                          {machineDisplayName(machine)}
                        </span>
                      </span>
                      <span className="truncate font-mono text-[10px] text-muted-foreground">
                        {session.provider}
                      </span>
                      <StatusDot
                        tone={status.tone}
                        label={status.label}
                        pulse={status.tone === 'running'}
                      />
                      <span
                        className="text-right font-mono text-[10px] text-faint"
                        title={formatDateTime(session.updated_at)}
                      >
                        {formatCompactRelativeTime(session.updated_at)}
                      </span>
                    </Link>
                  );
                })}
              </div>
            )}
          </Panel>
        ) : null}

        <AddWorkspaceDialog
          open={addWorkspaceOpen}
          onOpenChange={setAddWorkspaceOpen}
          machines={machines}
        />

        {pairOpen ? <PairMachineDialog orgId={orgId} onClose={() => setPairOpen(false)} /> : null}
      </div>
    </div>
  );
}

import type {
  AgentSession,
  DriverInfo,
  Machine,
  ProviderInfo,
  Workspace,
} from '@agent-fleet/protocol-types';
import { Link, useNavigate, useParams } from '@tanstack/react-router';
import { ArrowRight, GitBranch, Plus, Trash2 } from 'lucide-react';
import { useMemo, useState, type ReactNode } from 'react';
import { AddWorkspaceDialog } from '@/components/dialogs/add-workspace-dialog';
import { ConfirmDialog } from '@/components/dialogs/confirm-dialog';
import { NewSessionDialog } from '@/components/dialogs/new-session-dialog';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { StatusDot } from '@/components/ui/status-dot';
import { useDeleteMachine, useDeleteWorkspace } from '@/lib/query/mutations';
import {
  useMachine,
  useMachineDrivers,
  useMachineProviders,
  useSessionList,
  useWorkspacesForMachine,
} from '@/lib/query/hooks';
import { sessionIsActive, sessionStatusMeta } from '@/lib/status';
import { formatDateTime, getErrorMessage, shortId } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';
import {
  CapabilityList,
  formatCompactRelativeAgo,
  formatCompactRelativeTime,
  machineIsOnline,
  MachineStatusIndicator,
  PageKicker,
  Panel,
  PanelHeading,
  pageContainer,
} from './machine-ui';

export function MachineDetailPage() {
  const params = useParams({ strict: false });
  const machineId = typeof params.machineId === 'string' ? params.machineId : '';
  const navigate = useNavigate();

  const machineQuery = useMachine(machineId);
  const machine = machineQuery.data;
  const online = machineIsOnline(machine?.status);

  const providersQuery = useMachineProviders(machineId, Boolean(machine && online));
  const driversQuery = useMachineDrivers(machineId, Boolean(machine && online));
  const workspacesQuery = useWorkspacesForMachine(machineId);
  const sessionsQuery = useSessionList();
  const deleteMutation = useDeleteMachine();
  const deleteWorkspaceMutation = useDeleteWorkspace();

  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [addWorkspaceOpen, setAddWorkspaceOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [workspaceToDelete, setWorkspaceToDelete] = useState<{ id: string; name: string } | null>(
    null,
  );

  const providers = online
    ? (providersQuery.data ?? machine?.providers ?? [])
    : (machine?.providers ?? []);
  const workspaces = workspacesQuery.data ?? machine?.workspaces ?? [];

  const machineSessions = useMemo(
    () =>
      [...(sessionsQuery.data ?? [])]
        .filter((session) => session.machine_id === machineId)
        .sort((left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at)),
    [sessionsQuery.data, machineId],
  );

  const sessionsByWorkspace = useMemo(() => {
    const result = new Map<string, AgentSession[]>();
    for (const session of machineSessions) {
      if (!session.workspace_id) continue;
      const list = result.get(session.workspace_id);
      if (list) {
        list.push(session);
      } else {
        result.set(session.workspace_id, [session]);
      }
    }
    return result;
  }, [machineSessions]);

  const availableProviders = providers.filter((provider) => provider.available);
  const activeSessionCount = machineSessions.filter((session) =>
    sessionIsActive(session.status),
  ).length;

  if (machineQuery.isLoading) {
    return (
      <div className="h-full overflow-y-auto">
        <div className={pageContainer}>
          <LoadingState label="Loading machine…" />
        </div>
      </div>
    );
  }

  if (machineQuery.isError && !machineQuery.data) {
    return (
      <div className="h-full overflow-y-auto">
        <div className={pageContainer}>
          <Panel>
            <ErrorState
              title="Could not load machine"
              message={getErrorMessage(machineQuery.error)}
              onRetry={() => void machineQuery.refetch()}
            />
          </Panel>
        </div>
      </div>
    );
  }

  if (!machine) {
    return (
      <div className="h-full overflow-y-auto">
        <div className={pageContainer}>
          <Panel>
            <EmptyState
              title="Machine not found"
              description="This machine may have been deleted or is no longer registered with the fleet."
              action={
                <Link
                  to="/machines"
                  className="text-[12px] font-bold text-primary transition-colors hover:text-primary-hover"
                >
                  Back to machines
                </Link>
              }
            />
          </Panel>
        </div>
      </div>
    );
  }

  const newSessionDisabledReason = !online
    ? 'The daemon is offline. Start it before creating a session.'
    : availableProviders.length === 0
      ? 'No provider is available on this machine.'
      : workspaces.length === 0
        ? 'Register a workspace before starting a session.'
        : undefined;

  return (
    <div className="h-full overflow-y-auto">
      <div className={pageContainer}>
        <header className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <PageKicker className="flex flex-wrap items-center gap-1.5">
              <Link to="/machines" className="transition-colors hover:text-foreground">
                Machines
              </Link>
              <span aria-hidden="true">/</span>
              <span className="text-muted-foreground">{machineDisplayName(machine)}</span>
            </PageKicker>
            <div className="mt-1 flex min-w-0 items-center gap-2.5">
              <h1 className="truncate font-display text-[24px] font-bold leading-8 tracking-[-0.015em] text-foreground">
                {machineDisplayName(machine)}
              </h1>
              <MachineStatusIndicator status={machine.status} />
            </div>
            <p className="mt-1 font-mono text-[10px] leading-4 text-muted-foreground">
              {machine.os} / {machine.arch}
              <span className="mx-1.5 text-faint">·</span>
              daemon v{machine.daemon_version}
              <span className="mx-1.5 text-faint">·</span>
              last seen {formatCompactRelativeAgo(machine.last_seen_at)}
            </p>
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <Button variant="secondary" onClick={() => setAddWorkspaceOpen(true)}>
              <Plus className="size-3.5" aria-hidden="true" />
              Add workspace
            </Button>
            <Button
              variant="primary"
              onClick={() => setNewSessionOpen(true)}
              disabled={Boolean(newSessionDisabledReason)}
              title={newSessionDisabledReason}
            >
              New session
            </Button>
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Revoke machine ${machineDisplayName(machine)}`}
              title="Revoke machine"
              onClick={() => {
                deleteMutation.reset();
                setDeleteOpen(true);
              }}
              className="text-destructive hover:bg-destructive-soft hover:text-destructive"
            >
              <Trash2 className="size-4" aria-hidden="true" />
            </Button>
          </div>
        </header>

        <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,1fr)_456px]">
          <div className="flex min-w-0 flex-col gap-6">
            <ProvidersPanel
              providers={providers}
              availableCount={availableProviders.length}
              offline={!online}
              isLoading={providersQuery.isLoading}
              isError={providersQuery.isError}
              onRetry={() => void providersQuery.refetch()}
            />

            <DriversPanel
              drivers={driversQuery.data ?? []}
              offline={!online}
              isLoading={driversQuery.isLoading}
              isError={driversQuery.isError}
              onRetry={() => void driversQuery.refetch()}
            />

            <WorkspacesPanel
              workspaces={workspaces}
              sessionsByWorkspace={sessionsByWorkspace}
              isLoading={workspacesQuery.isLoading}
              isError={workspacesQuery.isError}
              onRetry={() => void workspacesQuery.refetch()}
              onAddWorkspace={() => setAddWorkspaceOpen(true)}
              onDeleteWorkspace={(workspace) => {
                deleteWorkspaceMutation.reset();
                setWorkspaceToDelete({ id: workspace.id, name: workspace.name });
              }}
            />

            <RecentSessionsPanel
              sessions={machineSessions.slice(0, 5)}
              isLoading={sessionsQuery.isLoading}
              isError={sessionsQuery.isError}
              errorMessage={sessionsQuery.error ? getErrorMessage(sessionsQuery.error) : undefined}
              onRetry={() => void sessionsQuery.refetch()}
            />
          </div>

          <HealthPanel
            machine={machine}
            online={online}
            workspaceCount={workspaces.length}
            providerCount={providers.length}
            availableProviderCount={availableProviders.length}
            sessionCount={machineSessions.length}
            activeSessionCount={activeSessionCount}
            probeLoading={providersQuery.isLoading}
            probeFetching={providersQuery.isFetching}
            probeError={providersQuery.isError}
          />
        </div>

        <NewSessionDialog
          key={`new-session:${machine.id}`}
          open={newSessionOpen}
          onOpenChange={setNewSessionOpen}
          machines={[machine]}
          defaultMachineId={machine.id}
          onCreated={(session) => {
            setNewSessionOpen(false);
            void navigate({
              to: '/sessions/$sessionId',
              params: { sessionId: session.id },
            });
          }}
        />

        <AddWorkspaceDialog
          key={`add-workspace:${machine.id}`}
          open={addWorkspaceOpen}
          onOpenChange={setAddWorkspaceOpen}
          machines={[machine]}
          defaultMachineId={machine.id}
        />

        <ConfirmDialog
          open={deleteOpen}
          onOpenChange={(open) => {
            if (deleteMutation.isPending) return;
            setDeleteOpen(open);
            if (!open) deleteMutation.reset();
          }}
          title="Revoke machine?"
          description={`${machineDisplayName(machine)} will be permanently revoked and removed from the active fleet. Its workspaces and sessions will no longer be addressable here. You can restore it later from the Revoked machines panel on the Machines page.`}
          confirmLabel="Revoke machine"
          danger
          pending={deleteMutation.isPending}
          error={deleteMutation.error ? getErrorMessage(deleteMutation.error) : undefined}
          onConfirm={() => {
            deleteMutation.mutate(machine, {
              onSuccess: () => {
                setDeleteOpen(false);
                void navigate({ to: '/machines' });
              },
            });
          }}
        />

        <ConfirmDialog
          open={Boolean(workspaceToDelete)}
          onOpenChange={(open) => {
            if (deleteWorkspaceMutation.isPending) return;
            if (!open) {
              setWorkspaceToDelete(null);
              deleteWorkspaceMutation.reset();
            }
          }}
          title="Delete workspace?"
          description={
            workspaceToDelete
              ? `${workspaceToDelete.name} will be removed from this machine. Files stay on disk. Workspaces with active sessions cannot be deleted; close those sessions first.`
              : undefined
          }
          confirmLabel="Delete workspace"
          danger
          pending={deleteWorkspaceMutation.isPending}
          error={
            deleteWorkspaceMutation.error
              ? getErrorMessage(deleteWorkspaceMutation.error)
              : undefined
          }
          onConfirm={() => {
            if (!workspaceToDelete) return;
            deleteWorkspaceMutation.mutate(workspaceToDelete, {
              onSuccess: () => setWorkspaceToDelete(null),
            });
          }}
        />
      </div>
    </div>
  );
}

function ProvidersPanel({
  providers,
  availableCount,
  offline,
  isLoading,
  isError,
  onRetry,
}: {
  providers: ProviderInfo[];
  availableCount: number;
  offline: boolean;
  isLoading: boolean;
  isError: boolean;
  onRetry: () => void;
}) {
  return (
    <Panel>
      <PanelHeading
        title="Providers"
        trailing={
          <span className="font-mono text-[9px] text-faint">{availableCount} available</span>
        }
      />

      {offline ? (
        <p className="border-b border-border px-5 py-2.5 text-[11px] leading-5 text-muted-foreground">
          Live probing is paused while the daemon is offline. Showing the last advertised provider
          capabilities.
        </p>
      ) : isError ? (
        <div className="flex items-center justify-between gap-3 border-b border-border px-5 py-2.5 text-[11px] leading-5 text-muted-foreground">
          <span>Live provider probe failed. Showing cached capabilities.</span>
          <button
            type="button"
            onClick={onRetry}
            className="shrink-0 text-[11px] font-bold text-primary transition-colors hover:text-primary-hover"
          >
            Retry
          </button>
        </div>
      ) : null}

      {isLoading && providers.length === 0 ? (
        <LoadingState label="Probing providers…" className="py-10" />
      ) : providers.length === 0 ? (
        <EmptyState
          title="No providers reported"
          description="This daemon has not advertised any coding-agent provider capabilities."
        />
      ) : (
        <div className="divide-y divide-border">
          {providers.map((provider) => (
            <ProviderRow key={provider.id} provider={provider} />
          ))}
        </div>
      )}
    </Panel>
  );
}

function DriversPanel({
  drivers,
  offline,
  isLoading,
  isError,
  onRetry,
}: {
  drivers: DriverInfo[];
  offline: boolean;
  isLoading: boolean;
  isError: boolean;
  onRetry: () => void;
}) {
  return (
    <Panel>
      <PanelHeading
        title="Drivers"
        trailing={<span className="font-mono text-[9px] text-faint">{drivers.length}</span>}
      />
      {offline && drivers.length === 0 ? (
        <p className="px-5 py-2.5 text-[11px] leading-5 text-muted-foreground">
          Driver discovery is live-only while the daemon is offline.
        </p>
      ) : isError ? (
        <div className="flex items-center justify-between gap-3 border-b border-border px-5 py-2.5 text-[11px] leading-5 text-muted-foreground">
          <span>Driver catalog failed to load.</span>
          <button
            type="button"
            onClick={onRetry}
            className="shrink-0 text-[11px] font-bold text-primary transition-colors hover:text-primary-hover"
          >
            Retry
          </button>
        </div>
      ) : null}
      {isLoading && drivers.length === 0 ? (
        <LoadingState label="Loading drivers…" className="py-10" />
      ) : drivers.length === 0 ? (
        offline ? null : (
          <EmptyState
            title="No drivers registered"
            description="The daemon did not report any agent drivers."
          />
        )
      ) : (
        <div className="divide-y divide-border">
          {drivers.map((driver) => (
            <div
              key={driver.id}
              className="flex items-center justify-between gap-4 px-5 py-3"
            >
              <div className="flex min-w-0 flex-col">
                <span className="truncate text-[13px] font-bold text-foreground">
                  {driver.display_name}
                </span>
                <span className="font-mono text-[9px] text-faint">{driver.id}</span>
              </div>
              <span className="shrink-0 rounded border border-border px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-wider text-muted-foreground">
                {driver.protocol}
              </span>
            </div>
          ))}
        </div>
      )}
    </Panel>
  );
}

function ProviderRow({ provider }: { provider: ProviderInfo }) {
  return (
    <div className="flex flex-col gap-1.5 px-5 py-3.5">
      <div className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 items-baseline gap-2">
          <span className="truncate text-[13px] font-bold text-foreground">
            {provider.display_name}
          </span>
          {provider.version ? (
            <span className="shrink-0 font-mono text-[9.5px] text-faint">{provider.version}</span>
          ) : null}
        </div>
        <StatusDot
          tone={provider.available ? 'running' : 'idle'}
          label={provider.available ? 'Available' : 'Unavailable'}
        />
      </div>
      <CapabilityList capabilities={provider.capabilities} />
      {provider.models?.length ? (
        <p className="truncate font-mono text-[9px] leading-4 text-faint">
          models: {provider.models.join(' · ')}
        </p>
      ) : null}
    </div>
  );
}

type WorkspaceView = Pick<Workspace, 'id' | 'name' | 'root_path' | 'is_git_repository'>;

function WorkspacesPanel({
  workspaces,
  sessionsByWorkspace,
  isLoading,
  isError,
  onRetry,
  onAddWorkspace,
  onDeleteWorkspace,
}: {
  workspaces: WorkspaceView[];
  sessionsByWorkspace: Map<string, AgentSession[]>;
  isLoading: boolean;
  isError: boolean;
  onRetry: () => void;
  onAddWorkspace: () => void;
  onDeleteWorkspace: (workspace: WorkspaceView) => void;
}) {
  return (
    <Panel>
      <PanelHeading
        title="Workspaces"
        trailing={
          <span className="font-mono text-[9px] text-faint">
            {workspaces.length} {workspaces.length === 1 ? 'workspace' : 'workspaces'}
          </span>
        }
      />

      {isError ? (
        <div className="flex items-center justify-between gap-3 border-b border-border px-5 py-2.5 text-[11px] leading-5 text-muted-foreground">
          <span>Live workspace list unavailable. Showing cached registrations.</span>
          <button
            type="button"
            onClick={onRetry}
            className="shrink-0 text-[11px] font-bold text-primary transition-colors hover:text-primary-hover"
          >
            Retry
          </button>
        </div>
      ) : null}

      {isLoading && workspaces.length === 0 ? (
        <LoadingState label="Loading workspaces…" className="py-10" />
      ) : workspaces.length === 0 ? (
        <EmptyState
          title="No workspaces registered"
          description="Register an existing directory on this machine to browse files, inspect Git changes and start sessions there."
          action={
            <Button variant="secondary" size="sm" onClick={onAddWorkspace}>
              Add workspace
            </Button>
          }
        />
      ) : (
        <div className="divide-y divide-border">
          {workspaces.map((workspace) => {
            const workspaceSessions = sessionsByWorkspace.get(workspace.id) ?? [];
            const activeCount = workspaceSessions.filter((session) =>
              sessionIsActive(session.status),
            ).length;

            return (
              <div
                key={workspace.id}
                className="flex items-center justify-between gap-4 px-5 py-3.5"
              >
                <div className="flex min-w-0 items-start gap-2.5">
                  <GitBranch
                    className="mt-0.5 size-3.5 shrink-0 text-muted-foreground"
                    aria-hidden="true"
                  />
                  <div className="min-w-0">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="truncate text-[13px] font-bold text-foreground">
                        {workspace.name}
                      </span>
                      <span className="shrink-0 font-mono text-[8.5px] uppercase tracking-[0.08em] text-faint">
                        {workspace.is_git_repository ? 'Git' : 'No git'}
                      </span>
                    </div>
                    <p className="truncate font-mono text-[9.5px] leading-4 text-faint">
                      {workspace.root_path}
                    </p>
                  </div>
                </div>

                <div className="flex shrink-0 items-center gap-3">
                  <div className="text-right">
                    <span className="block text-[11.5px] font-bold leading-5 text-foreground">
                      {workspaceSessions.length}{' '}
                      {workspaceSessions.length === 1 ? 'session' : 'sessions'}
                    </span>
                    {activeCount > 0 ? (
                      <span className="mt-0.5 flex justify-end">
                        <StatusDot tone="running" size="xs" label={`${activeCount} active`} pulse />
                      </span>
                    ) : (
                      <span className="mt-0.5 block font-mono text-[9px] leading-4 text-faint">
                        idle
                      </span>
                    )}
                  </div>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Delete workspace ${workspace.name}`}
                    title="Delete workspace"
                    className="text-destructive hover:bg-destructive-soft hover:text-destructive"
                    onClick={() => onDeleteWorkspace(workspace)}
                  >
                    <Trash2 className="size-3.5" aria-hidden="true" />
                  </Button>
                  <Link
                    to="/workspaces/$workspaceId"
                    params={{ workspaceId: workspace.id }}
                    className="inline-flex h-7 items-center rounded-md px-2 text-[11.5px] font-bold text-primary transition-colors hover:bg-primary-soft hover:text-primary-hover"
                  >
                    Open
                  </Link>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </Panel>
  );
}

function RecentSessionsPanel({
  sessions,
  isLoading,
  isError,
  errorMessage,
  onRetry,
}: {
  sessions: AgentSession[];
  isLoading: boolean;
  isError: boolean;
  errorMessage?: string;
  onRetry: () => void;
}) {
  return (
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

      {isError && sessions.length === 0 ? (
        <ErrorState
          title="Could not load sessions"
          message={errorMessage ?? 'Sessions could not be loaded.'}
          onRetry={onRetry}
          className="py-10"
        />
      ) : isLoading && sessions.length === 0 ? (
        <LoadingState label="Loading recent sessions…" className="py-10" />
      ) : sessions.length === 0 ? (
        <EmptyState
          title="No sessions yet"
          description="Start a session on this machine and it will appear here with its provider and live status."
        />
      ) : (
        <div>
          {sessions.map((session) => {
            const status = sessionStatusMeta(session.status);
            return (
              <Link
                key={session.id}
                to="/sessions/$sessionId"
                params={{ sessionId: session.id }}
                className="grid grid-cols-[minmax(0,1.8fr)_minmax(0,1fr)_minmax(0,1fr)_80px] items-center gap-4 border-t border-border px-5 py-3 transition-colors first:border-t-0 hover:bg-muted"
              >
                <span className="truncate text-[13px] font-bold text-foreground">
                  {session.title || `Session ${shortId(session.id, 8)}`}
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
  );
}

function HealthPanel({
  machine,
  online,
  workspaceCount,
  providerCount,
  availableProviderCount,
  sessionCount,
  activeSessionCount,
  probeLoading,
  probeFetching,
  probeError,
}: {
  machine: Machine;
  online: boolean;
  workspaceCount: number;
  providerCount: number;
  availableProviderCount: number;
  sessionCount: number;
  activeSessionCount: number;
  probeLoading: boolean;
  probeFetching: boolean;
  probeError: boolean;
}) {
  const probe = !online
    ? { tone: 'offline' as const, label: 'Paused' }
    : probeLoading
      ? { tone: 'waiting' as const, label: 'Probing' }
      : probeError
        ? { tone: 'failed' as const, label: 'Probe failed' }
        : probeFetching
          ? { tone: 'waiting' as const, label: 'Refreshing' }
          : { tone: 'running' as const, label: 'Live' };

  return (
    <Panel className="xl:sticky xl:top-4">
      <PanelHeading title="Health / Connection" />
      <div className="divide-y divide-border">
        <HealthRow label="Status">
          <MachineStatusIndicator status={machine.status} />
        </HealthRow>
        <HealthRow label="Daemon">
          <span className="font-mono text-[11px]">v{machine.daemon_version}</span>
        </HealthRow>
        <HealthRow label="Protocol">
          <span className="font-mono text-[11px]">v{machine.protocol_version}</span>
        </HealthRow>
        <HealthRow label="Last seen">
          <span className="font-mono text-[10.5px]">{formatDateTime(machine.last_seen_at)}</span>
        </HealthRow>
        <HealthRow label="Connected">
          <span className="font-mono text-[10.5px]">{formatDateTime(machine.connected_at)}</span>
        </HealthRow>
        <HealthRow label="Workspaces">
          <span className="font-bold tabular-nums text-foreground">{workspaceCount}</span>
        </HealthRow>
        <HealthRow label="Providers">
          <span className="font-bold tabular-nums text-foreground">{availableProviderCount}</span>{' '}
          <span className="text-faint">/ {providerCount}</span>
        </HealthRow>
        <HealthRow label="Sessions">
          <span className="font-bold tabular-nums text-foreground">{activeSessionCount}</span>{' '}
          <span className="text-faint">active / {sessionCount} total</span>
        </HealthRow>
        <HealthRow label="Provider probe">
          <StatusDot tone={probe.tone} label={probe.label} pulse={probe.tone === 'running'} />
        </HealthRow>
      </div>
      <p className="border-t border-border px-5 py-3.5 text-[11px] leading-5 text-muted-foreground">
        This panel reflects the latest machine snapshot and provider probe. Heartbeat history, CPU
        and memory usage are not reported over this protocol.
      </p>
    </Panel>
  );
}

function HealthRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 px-5 py-2.5">
      <span className="shrink-0 font-mono text-[9px] uppercase tracking-[0.1em] text-faint">
        {label}
      </span>
      <span className="min-w-0 truncate text-right text-[11.5px] text-muted-foreground">
        {children}
      </span>
    </div>
  );
}

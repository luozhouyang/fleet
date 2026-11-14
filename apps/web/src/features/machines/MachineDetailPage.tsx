import { MachineStatus } from '@agent-fleet/protocol-types';
import {
  Badge,
  Button,
  Card,
  CardHeader,
  EmptyState,
  PageHeader,
  Spinner,
} from '@agent-fleet/ui';
import { Link, useNavigate, useParams } from '@tanstack/react-router';
import { useState } from 'react';
import { AddWorkspaceDialog } from '@/components/dialogs/AddWorkspaceDialog';
import { NewSessionDialog } from '@/components/dialogs/NewSessionDialog';
import { ErrorState } from '@/components/feedback/ErrorState';
import { LoadingState } from '@/components/feedback/LoadingState';
import { MachineStatusBadge } from '@/components/status/StatusBadge';
import {
  useMachine,
  useMachineProviders,
  useWorkspacesForMachine,
} from '@/lib/query/hooks';
import { formatDateTime, formatRelativeTime } from '@/lib/utils/format';
import { ProviderCard } from './ProviderCard';

export function MachineDetailPage() {
  const params = useParams({ strict: false });
  const machineId = typeof params.machineId === 'string' ? params.machineId : '';
  const navigate = useNavigate();
  const machineQuery = useMachine(machineId);
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [addWorkspaceOpen, setAddWorkspaceOpen] = useState(false);

  const machine = machineQuery.data;
  const live =
    machine?.status === MachineStatus.Online || machine?.status === MachineStatus.Degraded;
  const providersQuery = useMachineProviders(machineId, Boolean(machine && live));
  const workspacesQuery = useWorkspacesForMachine(machineId);
  const providers = providersQuery.data ?? machine?.providers ?? [];
  const workspaces = workspacesQuery.data ?? machine?.workspaces ?? [];

  if (machineQuery.isLoading) return <LoadingState label="Loading machine…" />;
  if (machineQuery.isError) {
    return (
      <ErrorState
        error={machineQuery.error}
        title="Could not load machine"
        onRetry={() => void machineQuery.refetch()}
      />
    );
  }
  if (!machine) {
    return (
      <Card>
        <EmptyState
          title="Machine not found"
          description="This machine may have been deleted or is no longer registered."
          action={
            <Link
              to="/machines"
              className="text-xs font-medium text-teal-300 hover:text-teal-200"
            >
              Back to machines
            </Link>
          }
        />
      </Card>
    );
  }

  const availableProviders = providers.filter((provider) => provider.available);

  return (
    <div className="space-y-6">
      <PageHeader
        eyebrow={
          <span className="flex items-center gap-2">
            <Link to="/machines" className="hover:text-teal-200">
              Machines
            </Link>
            <span className="text-slate-700">/</span>
            <span>{machine.hostname}</span>
          </span>
        }
        title={machine.name}
        description={`${machine.os} / ${machine.arch} · daemon v${machine.daemon_version} · last seen ${formatRelativeTime(machine.last_seen_at)}`}
        actions={
          <>
            <MachineStatusBadge status={machine.status} />
            <Button variant="secondary" onClick={() => setAddWorkspaceOpen(true)}>
              Add workspace
            </Button>
            <Button
              variant="primary"
              onClick={() => setNewSessionOpen(true)}
              disabled={!availableProviders.length || workspaces.length === 0}
              title={
                availableProviders.length === 0
                  ? 'No available provider is registered on this machine'
                  : workspaces.length === 0
                    ? 'Register a workspace before starting a session'
                    : undefined
              }
            >
              New session
            </Button>
          </>
        }
      />

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
        <Card>
          <CardHeader
            title="Machine profile"
            description="Identity and daemon metadata reported during connection."
          />
          <div className="grid grid-cols-2 gap-x-6 gap-y-4 p-4 sm:grid-cols-3">
            <Detail label="Hostname" value={machine.hostname} mono />
            <Detail label="Platform" value={`${machine.os} / ${machine.arch}`} mono />
            <Detail label="Daemon" value={`v${machine.daemon_version}`} mono />
            <Detail label="Protocol" value={`v${machine.protocol_version}`} mono />
            <Detail label="Last seen" value={formatDateTime(machine.last_seen_at)} />
            <Detail label="Connected" value={formatDateTime(machine.connected_at)} />
          </div>
        </Card>

        <Card>
          <CardHeader
            title="Connection"
            description="Live daemon state and counts."
            action={
              providersQuery.isFetching ? (
                <Spinner size="sm" label="Refreshing providers" />
              ) : null
            }
          />
          <div className="space-y-3 p-4">
            <div className="flex items-center justify-between rounded-lg border border-slate-800 bg-slate-950/40 px-3 py-2.5">
              <span className="text-xs text-slate-500">Status</span>
              <MachineStatusBadge status={machine.status} />
            </div>
            <div className="flex items-center justify-between rounded-lg border border-slate-800 bg-slate-950/40 px-3 py-2.5">
              <span className="text-xs text-slate-500">Workspaces</span>
              <span className="text-sm tabular-nums text-slate-200">{workspaces.length}</span>
            </div>
            <div className="flex items-center justify-between rounded-lg border border-slate-800 bg-slate-950/40 px-3 py-2.5">
              <span className="text-xs text-slate-500">Providers</span>
              <span className="text-sm tabular-nums text-slate-200">
                {availableProviders.length} available
              </span>
            </div>
            {!live ? (
              <p className="rounded-md border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-[11px] leading-5 text-amber-300/80">
                The daemon is offline. Provider probing is paused; last known capabilities are
                shown below.
              </p>
            ) : null}
          </div>
        </Card>
      </div>

      <section className="space-y-3">
        <div className="flex items-end justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold text-slate-100">Providers &amp; capabilities</h2>
            <p className="mt-0.5 text-xs text-slate-500">
              Only capabilities the adapter actually implements are marked as supported.
            </p>
          </div>
          {providersQuery.isError ? (
            <span className="text-[11px] text-amber-400/80">
              Live provider probe failed; showing cached data.
            </span>
          ) : null}
        </div>
        {providersQuery.isLoading && providers.length === 0 ? (
          <Card className="flex min-h-32 items-center justify-center gap-3 text-xs text-slate-500">
            <Spinner size="sm" label="Loading providers" />
            Loading provider capabilities…
          </Card>
        ) : providers.length ? (
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {providers.map((provider) => (
              <ProviderCard key={provider.id} provider={provider} />
            ))}
          </div>
        ) : (
          <Card>
            <EmptyState
              title="No providers reported"
              description="This daemon has not advertised any coding-agent provider capabilities."
            />
          </Card>
        )}
      </section>

      <section className="space-y-3">
        <div className="flex items-end justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold text-slate-100">Workspaces</h2>
            <p className="mt-0.5 text-xs text-slate-500">
              Registered roots available to files, Git and agent sessions on this machine.
            </p>
          </div>
          {workspacesQuery.isError ? (
            <button
              type="button"
              onClick={() => void workspacesQuery.refetch()}
              className="text-[11px] text-amber-400/80 hover:text-amber-300"
            >
              Using cached list · retry
            </button>
          ) : null}
        </div>

        {workspacesQuery.isLoading && workspaces.length === 0 ? (
          <Card className="flex min-h-32 items-center justify-center gap-3 text-xs text-slate-500">
            <Spinner size="sm" label="Loading workspaces" />
            Loading workspaces…
          </Card>
        ) : workspaces.length ? (
          <Card className="divide-y divide-slate-800/70 overflow-hidden">
            {workspaces.map((workspace) => (
              <div
                key={workspace.id}
                className="flex flex-col gap-3 p-4 transition-colors hover:bg-slate-900/50 sm:flex-row sm:items-center sm:justify-between"
              >
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-sm font-medium text-slate-100">{workspace.name}</span>
                    {workspace.is_git_repository ? (
                      <Badge tone="teal">Git</Badge>
                    ) : (
                      <Badge tone="slate">No Git</Badge>
                    )}
                  </div>
                  <p className="mt-1 truncate font-mono text-[11px] text-slate-500">
                    {workspace.root_path}
                  </p>
                </div>
                <Link
                  to="/workspaces/$workspaceId"
                  params={{ workspaceId: workspace.id }}
                  className="inline-flex h-8 shrink-0 items-center justify-center rounded-md border border-slate-700 bg-slate-900 px-3 text-xs font-medium text-slate-200 transition-colors hover:border-slate-600 hover:bg-slate-800"
                >
                  Open explorer
                </Link>
              </div>
            ))}
          </Card>
        ) : (
          <Card>
            <EmptyState
              title="No workspaces registered"
              description="Register an existing directory on this machine to browse files, inspect Git changes and start sessions there."
              action={
                <Button variant="secondary" size="sm" onClick={() => setAddWorkspaceOpen(true)}>
                  Add workspace
                </Button>
              }
            />
          </Card>
        )}
      </section>

      <NewSessionDialog
        open={newSessionOpen}
        machine={machine}
        providers={providers}
        onCreated={(session) => {
          setNewSessionOpen(false);
          void navigate({
            to: '/sessions/$sessionId',
            params: { sessionId: session.id },
          });
        }}
        onClose={() => setNewSessionOpen(false)}
      />

      <AddWorkspaceDialog
        open={addWorkspaceOpen}
        machines={[machine]}
        defaultMachineId={machine.id}
        onClose={() => setAddWorkspaceOpen(false)}
      />
    </div>
  );
}

function Detail({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="min-w-0">
      <div className="text-[10px] uppercase tracking-[0.14em] text-slate-600">{label}</div>
      <div
        className={
          mono
            ? 'mt-1 truncate font-mono text-xs text-slate-300'
            : 'mt-1 truncate text-xs text-slate-300'
        }
      >
        {value}
      </div>
    </div>
  );
}

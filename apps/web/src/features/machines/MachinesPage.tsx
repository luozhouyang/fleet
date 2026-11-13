import {
  MachineStatus,
  type Machine,
} from '@agent-fleet/protocol-types';
import {
  Button,
  Card,
  EmptyState,
  PageHeader,
  Spinner,
} from '@agent-fleet/ui';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { AddWorkspaceDialog } from '@/components/dialogs/AddWorkspaceDialog';
import { ConfirmDialog } from '@/components/dialogs/ConfirmDialog';
import { ErrorState } from '@/components/feedback/ErrorState';
import { apiClient } from '@/lib/api/client';
import { useMachines } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { getErrorMessage } from '@/lib/utils/format';
import { MachineCard } from './MachineCard';

export function MachinesPage() {
  const machinesQuery = useMachines();
  const queryClient = useQueryClient();
  const [addWorkspaceMachineId, setAddWorkspaceMachineId] = useState<string>();
  const [addWorkspaceOpen, setAddWorkspaceOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Machine | null>(null);

  const deleteMutation = useMutation({
    mutationFn: (machine: Machine) => apiClient.deleteMachine(machine.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: ['workspaces'] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      setDeleteTarget(null);
    },
  });

  const machines = machinesQuery.data ?? [];
  const onlineCount = machines.filter(
    (machine) =>
      machine.status === MachineStatus.Online || machine.status === MachineStatus.Degraded,
  ).length;
  const workspaceCount = machines.reduce(
    (total, machine) =>
      total + Math.max(machine.workspace_count ?? 0, machine.workspaces?.length ?? 0),
    0,
  );
  const providerCount = new Set(
    machines.flatMap((machine) => (machine.providers ?? []).map((provider) => provider.id)),
  ).size;

  const openAddWorkspace = (machine?: Machine) => {
    setAddWorkspaceMachineId(machine?.id);
    setAddWorkspaceOpen(true);
  };

  return (
    <div className="space-y-6">
      <PageHeader
        eyebrow="Fleet"
        title="Machines"
        description="Registered daemons, their workspaces and the coding-agent providers they expose."
        actions={
          <>
            <span className="hidden items-center gap-2 text-xs text-slate-500 sm:flex">
              {machinesQuery.isFetching ? (
                <Spinner size="sm" label="Refreshing machines" />
              ) : (
                <span className="size-1.5 rounded-full bg-slate-600" />
              )}
              Auto-refresh 15s
            </span>
            <Button
              variant="primary"
              onClick={() => openAddWorkspace()}
              disabled={!machines.length}
              title={machines.length ? undefined : 'Connect a machine first'}
            >
              Add workspace
            </Button>
          </>
        }
      />

      {machinesQuery.isError ? (
        <ErrorState
          error={machinesQuery.error}
          title="Could not load machines"
          onRetry={() => void machinesQuery.refetch()}
        />
      ) : machinesQuery.isLoading ? (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {Array.from({ length: 3 }).map((_, index) => (
            <Card key={index} className="flex h-72 items-center justify-center">
              <Spinner label="Loading machines" />
            </Card>
          ))}
        </div>
      ) : machines.length === 0 ? (
        <Card>
          <EmptyState
            title="No machines connected"
            description="Start an agent daemon and connect it to the control plane. The machine will appear here once it authenticates."
            icon={
              <svg viewBox="0 0 24 24" fill="none" className="size-5" aria-hidden="true">
                <rect x="3" y="4" width="18" height="7" rx="2" stroke="currentColor" strokeWidth="1.5" />
                <rect x="3" y="13" width="18" height="7" rx="2" stroke="currentColor" strokeWidth="1.5" />
                <path d="M7 7.5h.01M7 16.5h.01M11 7.5h5M11 16.5h5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
              </svg>
            }
          />
        </Card>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            <Stat label="Machines" value={machines.length} />
            <Stat label="Online" value={onlineCount} accent />
            <Stat label="Workspaces" value={workspaceCount} />
            <Stat label="Providers" value={providerCount} />
          </div>

          <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {machines.map((machine) => (
              <MachineCard
                key={machine.id}
                machine={machine}
                onDelete={setDeleteTarget}
                onAddWorkspace={(target) => openAddWorkspace(target)}
              />
            ))}
          </div>
        </>
      )}

      <AddWorkspaceDialog
        open={addWorkspaceOpen}
        machines={machines}
        defaultMachineId={addWorkspaceMachineId}
        onClose={() => setAddWorkspaceOpen(false)}
      />

      <ConfirmDialog
        open={Boolean(deleteTarget)}
        title="Delete machine?"
        description={
          deleteTarget
            ? `${deleteTarget.name} will be removed from the control plane. Its workspace registrations will no longer be addressable here.`
            : undefined
        }
        confirmLabel="Delete machine"
        danger
        pending={deleteMutation.isPending}
        error={deleteMutation.error ? getErrorMessage(deleteMutation.error) : undefined}
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget);
        }}
        onClose={() => {
          if (!deleteMutation.isPending) {
            setDeleteTarget(null);
            deleteMutation.reset();
          }
        }}
      >
        {deleteTarget ? (
          <div className="rounded-md border border-slate-800 bg-slate-950/40 p-3 text-xs text-slate-400">
            <span className="font-mono text-slate-300">{deleteTarget.hostname}</span>
            <span className="mx-2 text-slate-700">·</span>
            {deleteTarget.os}/{deleteTarget.arch}
          </div>
        ) : null}
      </ConfirmDialog>
    </div>
  );
}

function Stat({ label, value, accent = false }: { label: string; value: number; accent?: boolean }) {
  return (
    <Card className="px-4 py-3">
      <div className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-600">
        {label}
      </div>
      <div
        className={
          accent
            ? 'mt-1 text-xl font-semibold tabular-nums text-teal-300'
            : 'mt-1 text-xl font-semibold tabular-nums text-slate-100'
        }
      >
        {value}
      </div>
    </Card>
  );
}

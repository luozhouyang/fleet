import type { AgentSession, Machine } from '@agent-fleet/protocol-types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Select } from '@/components/ui/select';
import { apiClient } from '@/lib/api/client';
import { useMachineProviders } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { getErrorMessage } from '@/lib/utils/format';

export function NewSessionDialog({
  open,
  onOpenChange,
  machines,
  defaultMachineId,
  defaultWorkspaceId,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  machines: Machine[];
  defaultMachineId?: string;
  defaultWorkspaceId?: string;
  onCreated?: (session: AgentSession) => void;
}) {
  const queryClient = useQueryClient();
  const [machineId, setMachineId] = useState(defaultMachineId ?? machines[0]?.id ?? '');
  const [workspaceId, setWorkspaceId] = useState(defaultWorkspaceId ?? '');
  const [provider, setProvider] = useState('');
  const [title, setTitle] = useState('');
  const [model, setModel] = useState('');

  const machine = machines.find((candidate) => candidate.id === machineId);
  const providersQuery = useMachineProviders(machineId, open && Boolean(machineId));
  const providers = useMemo(
    () => providersQuery.data ?? machine?.providers ?? [],
    [providersQuery.data, machine],
  );
  const availableProviders = useMemo(
    () => providers.filter((candidate) => candidate.available),
    [providers],
  );
  const workspaces = useMemo(() => machine?.workspaces ?? [], [machine]);

  useEffect(() => {
    if (open && !machineId && machines[0]) setMachineId(machines[0].id);
  }, [open, machineId, machines]);

  useEffect(() => {
    if (!open) return;
    setTitle('');
    setModel('');
  }, [open]);

  useEffect(() => {
    if (!open || !machine) return;
    if (!workspaces.some((workspace) => workspace.id === workspaceId)) {
      setWorkspaceId(defaultWorkspaceId ?? workspaces[0]?.id ?? '');
    }
  }, [open, machine, workspaces, workspaceId, defaultWorkspaceId]);

  useEffect(() => {
    if (!open) return;
    if (!availableProviders.some((candidate) => candidate.id === provider)) {
      setProvider(availableProviders[0]?.id ?? '');
    }
  }, [open, availableProviders, provider]);

  const mutation = useMutation({
    mutationFn: () =>
      apiClient.createSession({
        machine_id: machineId,
        provider,
        workspace_id: workspaceId,
        cwd: workspaces.find((workspace) => workspace.id === workspaceId)?.root_path,
        title: title.trim() || undefined,
        model: model.trim() || undefined,
      }),
    onSuccess: (session) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      onCreated?.(session);
      onOpenChange(false);
    },
  });

  const selectedProvider = providers.find((candidate) => candidate.id === provider);
  const canStart = Boolean(machineId && provider && workspaceId) && !mutation.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader
          title="New session"
          description="Start a coding agent on one of your machines."
          onClose={() => onOpenChange(false)}
        />
        <form
          className="mt-5 flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            if (canStart) mutation.mutate();
          }}
        >
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="new-session-machine"
              className="text-[11px] font-bold text-muted-foreground"
            >
              Machine
            </label>
            <Select
              id="new-session-machine"
              value={machineId}
              onValueChange={(value) => {
                setMachineId(value);
                setWorkspaceId('');
                setProvider('');
              }}
              options={machines.map((candidate) => ({
                value: candidate.id,
                label: candidate.name,
                description: `${candidate.os} · ${candidate.status}`,
              }))}
              placeholder="Select a machine"
              disabled={mutation.isPending || machines.length === 0}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="new-session-provider"
              className="text-[11px] font-bold text-muted-foreground"
            >
              Provider
            </label>
            <Select
              id="new-session-provider"
              value={provider}
              onValueChange={setProvider}
              options={providers.map((candidate) => ({
                value: candidate.id,
                label: candidate.display_name,
                description: candidate.available
                  ? candidate.version
                    ? `v${candidate.version}`
                    : candidate.kind
                  : 'unavailable',
                disabled: !candidate.available,
              }))}
              placeholder={providers.length ? 'Select a provider' : 'No providers registered'}
              disabled={mutation.isPending || providers.length === 0}
            />
            {selectedProvider?.models?.length ? (
              <p className="font-mono text-[9px] text-faint">
                {selectedProvider.models.slice(0, 4).join(' · ')}
              </p>
            ) : null}
          </div>

          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="new-session-workspace"
              className="text-[11px] font-bold text-muted-foreground"
            >
              Workspace
            </label>
            <Select
              id="new-session-workspace"
              value={workspaceId}
              onValueChange={setWorkspaceId}
              options={workspaces.map((workspace) => ({
                value: workspace.id,
                label: workspace.name,
                description: workspace.root_path,
              }))}
              placeholder="Select a workspace"
              disabled={mutation.isPending || workspaces.length === 0}
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="new-session-title"
                className="text-[11px] font-bold text-muted-foreground"
              >
                Title
              </label>
              <Input
                id="new-session-title"
                value={title}
                onChange={(event) => setTitle(event.target.value)}
                placeholder="Optional label"
                disabled={mutation.isPending}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="new-session-model"
                className="text-[11px] font-bold text-muted-foreground"
              >
                Model
              </label>
              <Input
                id="new-session-model"
                value={model}
                onChange={(event) => setModel(event.target.value)}
                placeholder="Provider default"
                disabled={mutation.isPending}
              />
            </div>
          </div>

          {mutation.error ? (
            <p className="rounded-md border border-destructive/20 bg-destructive-soft px-3 py-2 text-[11.5px] text-destructive">
              {getErrorMessage(mutation.error)}
            </p>
          ) : null}

          <div className="mt-1 flex items-center justify-end gap-2">
            <Button
              variant="secondary"
              onClick={() => onOpenChange(false)}
              disabled={mutation.isPending}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={!canStart}>
              {mutation.isPending ? 'Starting…' : 'Start session'}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

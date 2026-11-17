import type {
  AgentSession,
  Machine,
  Workspace,
  WorkspaceSummary,
} from '@agent-fleet/protocol-types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Select } from '@/components/ui/select';
import { apiClient } from '@/lib/api/client';
import { useMachineProviders } from '@/lib/query/hooks';
import { useCreateWorkspace } from '@/lib/query/mutations';
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
  const [addingWorkspace, setAddingWorkspace] = useState(false);
  const [workspacePath, setWorkspacePath] = useState('');
  const [workspaceName, setWorkspaceName] = useState('');
  const [createdWorkspace, setCreatedWorkspace] = useState<Workspace | null>(null);
  const createWorkspace = useCreateWorkspace();
  const resetCreateWorkspace = createWorkspace.reset;

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
  // Keep the just-created workspace selectable until the machines query
  // refetches with the new snapshot.
  const workspaces = useMemo<WorkspaceSummary[]>(() => {
    const registered = machine?.workspaces ?? [];
    if (
      createdWorkspace &&
      machine &&
      createdWorkspace.machine_id === machine.id &&
      !registered.some((workspace) => workspace.id === createdWorkspace.id)
    ) {
      return [
        ...registered,
        {
          id: createdWorkspace.id,
          name: createdWorkspace.name,
          root_path: createdWorkspace.root_path,
          is_git_repository: createdWorkspace.is_git_repository,
        },
      ];
    }
    return registered;
  }, [machine, createdWorkspace]);

  useEffect(() => {
    if (open && !machineId && machines[0]) setMachineId(machines[0].id);
  }, [open, machineId, machines]);

  useEffect(() => {
    if (open) {
      setTitle('');
      setModel('');
      return;
    }
    setAddingWorkspace(false);
    setWorkspacePath('');
    setWorkspaceName('');
    setCreatedWorkspace(null);
    resetCreateWorkspace();
  }, [open, resetCreateWorkspace]);

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

  const canAddWorkspace = Boolean(machineId && workspacePath.trim()) && !createWorkspace.isPending;

  function createAndSelectWorkspace() {
    if (!canAddWorkspace) return;
    createWorkspace.mutate(
      {
        machineId,
        rootPath: workspacePath.trim(),
        name: workspaceName.trim() || undefined,
      },
      {
        onSuccess: (workspace) => {
          setCreatedWorkspace(workspace);
          setWorkspaceId(workspace.id);
          setAddingWorkspace(false);
          setWorkspacePath('');
          setWorkspaceName('');
        },
      },
    );
  }

  function cancelAddingWorkspace() {
    setAddingWorkspace(false);
    setWorkspacePath('');
    setWorkspaceName('');
    resetCreateWorkspace();
  }

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
                setCreatedWorkspace(null);
                cancelAddingWorkspace();
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
            <div className="flex items-center justify-between gap-2">
              <label
                htmlFor="new-session-workspace"
                className="text-[11px] font-bold text-muted-foreground"
              >
                Workspace
              </label>
              {!addingWorkspace ? (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-5 px-1.5 text-[10px]"
                  onClick={() => setAddingWorkspace(true)}
                  disabled={!machineId || mutation.isPending}
                >
                  Add workspace
                </Button>
              ) : null}
            </div>
            <Select
              id="new-session-workspace"
              value={workspaceId}
              onValueChange={setWorkspaceId}
              options={workspaces.map((workspace) => ({
                value: workspace.id,
                label: workspace.name,
                description: workspace.root_path,
              }))}
              placeholder={workspaces.length ? 'Select a workspace' : 'No workspaces yet'}
              disabled={mutation.isPending || addingWorkspace || workspaces.length === 0}
            />
            {workspaces.length === 0 && !addingWorkspace ? (
              <p className="text-[10px] leading-relaxed text-faint">
                No workspaces are registered on this machine. Add a directory to start a
                session.
              </p>
            ) : null}
            {addingWorkspace ? (
              <div className="flex flex-col gap-2 rounded-md border border-border bg-muted/30 p-2.5">
                <Input
                  aria-label="Workspace root path"
                  value={workspacePath}
                  onChange={(event) => setWorkspacePath(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault();
                      createAndSelectWorkspace();
                    }
                  }}
                  placeholder="/absolute/path/on/the/machine"
                  className="font-mono text-[11px]"
                  disabled={createWorkspace.isPending}
                />
                <Input
                  aria-label="Workspace name"
                  value={workspaceName}
                  onChange={(event) => setWorkspaceName(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault();
                      createAndSelectWorkspace();
                    }
                  }}
                  placeholder="Name (optional)"
                  disabled={createWorkspace.isPending}
                />
                {createWorkspace.error ? (
                  <p className="rounded-md border border-destructive/20 bg-destructive-soft px-2 py-1.5 text-[10.5px] text-destructive">
                    {getErrorMessage(createWorkspace.error)}
                  </p>
                ) : null}
                <div className="flex justify-end gap-2">
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={cancelAddingWorkspace}
                    disabled={createWorkspace.isPending}
                  >
                    Cancel
                  </Button>
                  <Button
                    variant="primary"
                    size="sm"
                    onClick={createAndSelectWorkspace}
                    disabled={!canAddWorkspace}
                  >
                    {createWorkspace.isPending ? 'Adding…' : 'Add workspace'}
                  </Button>
                </div>
              </div>
            ) : null}
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

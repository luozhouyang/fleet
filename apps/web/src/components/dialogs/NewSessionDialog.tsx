import type { AgentSession, Machine, ProviderInfo } from '@agent-fleet/protocol-types';
import { Button, Dialog, Field, Input, Select } from '@agent-fleet/ui';
import { useMutation } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { apiClient } from '@/lib/api/client';
import { getErrorMessage } from '@/lib/utils/format';

const EMPTY_WORKSPACES: NonNullable<Machine['workspaces']> = [];

export function NewSessionDialog({
  open,
  machine,
  providers,
  defaultWorkspaceId,
  onCreated,
  onClose,
}: {
  open: boolean;
  machine: Machine;
  providers: ProviderInfo[];
  defaultWorkspaceId?: string;
  onCreated: (session: AgentSession) => void;
  onClose: () => void;
}) {
  const id = useId();
  const workspaces = machine.workspaces ?? EMPTY_WORKSPACES;
  const availableProviders = providers.filter((provider) => provider.available);
  const firstProvider = availableProviders[0]?.id ?? providers[0]?.id ?? '';

  const [provider, setProvider] = useState(firstProvider);
  const [workspaceId, setWorkspaceId] = useState(
    defaultWorkspaceId ?? workspaces[0]?.id ?? '',
  );
  const [title, setTitle] = useState('');
  const [model, setModel] = useState('');
  const wasOpen = useRef(false);

  useEffect(() => {
    if (open && !wasOpen.current) {
      setProvider(firstProvider);
      setWorkspaceId(defaultWorkspaceId ?? workspaces[0]?.id ?? '');
      setTitle('');
      setModel('');
    }
    wasOpen.current = open;
  }, [open, firstProvider, defaultWorkspaceId, workspaces]);

  const selectedProvider = providers.find((candidate) => candidate.id === provider);
  const selectedWorkspace = workspaces.find((workspace) => workspace.id === workspaceId);

  const mutation = useMutation({
    mutationFn: () =>
      apiClient.createSession({
        machine_id: machine.id,
        provider,
        workspace_id: workspaceId,
        cwd: selectedWorkspace?.root_path,
        title: title.trim() || undefined,
        model: model.trim() || undefined,
      }),
    onSuccess: onCreated,
  });

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="New coding agent session"
      description={`Start a provider session on ${machine.name}.`}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={mutation.isPending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => mutation.mutate()}
            loading={mutation.isPending}
            disabled={!provider || !workspaceId || mutation.isPending}
            title={
              workspaceId
                ? undefined
                : 'Register and select a workspace before starting a session'
            }
          >
            Start session
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field
          label="Provider"
          htmlFor={`${id}-provider`}
          required
          hint={
            selectedProvider
              ? `${selectedProvider.display_name}${selectedProvider.version ? ` · v${selectedProvider.version}` : ''}`
              : 'No provider is registered on this machine.'
          }
        >
          <Select
            id={`${id}-provider`}
            value={provider}
            onChange={(event) => setProvider(event.target.value)}
            disabled={mutation.isPending || providers.length === 0}
          >
            {providers.length === 0 ? <option value="">No providers</option> : null}
            {providers.map((entry) => (
              <option key={entry.id} value={entry.id} disabled={!entry.available}>
                {entry.display_name}
                {entry.available ? '' : ' (unavailable)'}
              </option>
            ))}
          </Select>
        </Field>

        <Field
          label="Workspace"
          htmlFor={`${id}-workspace`}
          required
          hint={
            workspaces.length === 0
              ? 'Register a workspace on this machine first.'
              : 'Sessions run inside the selected workspace root.'
          }
        >
          <Select
            id={`${id}-workspace`}
            value={workspaceId}
            onChange={(event) => setWorkspaceId(event.target.value)}
            disabled={mutation.isPending || workspaces.length === 0}
          >
            {workspaces.length === 0 ? <option value="">No workspaces</option> : null}
            {workspaces.map((workspace) => (
              <option key={workspace.id} value={workspace.id}>
                {workspace.name} · {workspace.root_path}
              </option>
            ))}
          </Select>
        </Field>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Title" htmlFor={`${id}-title`} hint="Optional session label.">
            <Input
              id={`${id}-title`}
              value={title}
              onChange={(event) => setTitle(event.target.value)}
              placeholder="Refactor auth flow"
              disabled={mutation.isPending}
            />
          </Field>
          <Field label="Model" htmlFor={`${id}-model`} hint="Optional model override.">
            <Input
              id={`${id}-model`}
              value={model}
              onChange={(event) => setModel(event.target.value)}
              placeholder="default"
              disabled={mutation.isPending}
            />
          </Field>
        </div>

        {mutation.error ? (
          <p className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs leading-5 text-red-300">
            {getErrorMessage(mutation.error)}
          </p>
        ) : null}
      </div>
    </Dialog>
  );
}

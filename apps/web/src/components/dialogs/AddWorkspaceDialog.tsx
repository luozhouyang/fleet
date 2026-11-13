import type { Machine } from '@agent-fleet/protocol-types';
import { Button, Dialog, Field, Input, Select } from '@agent-fleet/ui';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { apiClient } from '@/lib/api/client';
import { queryKeys } from '@/lib/query/keys';
import { getErrorMessage } from '@/lib/utils/format';

export function AddWorkspaceDialog({
  open,
  machines,
  defaultMachineId,
  onClose,
}: {
  open: boolean;
  machines: Machine[];
  defaultMachineId?: string;
  onClose: () => void;
}) {
  const id = useId();
  const queryClient = useQueryClient();
  const firstMachineId = machines[0]?.id ?? '';
  const [machineId, setMachineId] = useState(defaultMachineId ?? firstMachineId);
  const [rootPath, setRootPath] = useState('');
  const [name, setName] = useState('');
  const wasOpen = useRef(false);

  useEffect(() => {
    if (open && !wasOpen.current) {
      setMachineId(defaultMachineId ?? firstMachineId);
      setRootPath('');
      setName('');
    }
    wasOpen.current = open;
  }, [open, defaultMachineId, firstMachineId]);

  const mutation = useMutation({
    mutationFn: () =>
      apiClient.createWorkspace({
        machine_id: machineId,
        root_path: rootPath.trim(),
        name: name.trim() || undefined,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: ['workspaces'] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      onClose();
    },
  });

  const canSubmit = Boolean(machineId && rootPath.trim()) && !mutation.isPending;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Add workspace"
      description="Register an existing directory on a connected machine."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={mutation.isPending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => mutation.mutate()}
            loading={mutation.isPending}
            disabled={!canSubmit}
          >
            Add workspace
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Machine" htmlFor={`${id}-machine`} required>
          <Select
            id={`${id}-machine`}
            value={machineId}
            onChange={(event) => setMachineId(event.target.value)}
            disabled={mutation.isPending}
          >
            {machines.length === 0 ? <option value="">No machines available</option> : null}
            {machines.map((machine) => (
              <option key={machine.id} value={machine.id}>
                {machine.name} · {machine.hostname}
              </option>
            ))}
          </Select>
        </Field>

        <Field
          label="Root path"
          htmlFor={`${id}-path`}
          required
          hint="Absolute path to an existing directory on the machine, e.g. /home/me/projects/app."
        >
          <Input
            id={`${id}-path`}
            value={rootPath}
            onChange={(event) => setRootPath(event.target.value)}
            placeholder="/home/me/projects/app"
            spellCheck={false}
            autoComplete="off"
            className="font-mono"
            disabled={mutation.isPending}
          />
        </Field>

        <Field
          label="Display name"
          htmlFor={`${id}-name`}
          hint="Optional. Defaults to the directory name."
        >
          <Input
            id={`${id}-name`}
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="my-project"
            disabled={mutation.isPending}
          />
        </Field>

        {mutation.error ? (
          <p className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs leading-5 text-red-300">
            {getErrorMessage(mutation.error)}
          </p>
        ) : null}
      </div>
    </Dialog>
  );
}

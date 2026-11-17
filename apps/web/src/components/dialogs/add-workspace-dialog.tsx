import type { Machine } from '@agent-fleet/protocol-types';
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Select } from '@/components/ui/select';
import { useCreateWorkspace } from '@/lib/query/mutations';
import { getErrorMessage } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';

export function AddWorkspaceDialog({
  open,
  onOpenChange,
  machines,
  defaultMachineId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  machines: Machine[];
  defaultMachineId?: string;
}) {
  const [machineId, setMachineId] = useState(defaultMachineId ?? machines[0]?.id ?? '');
  const [name, setName] = useState('');
  const [rootPath, setRootPath] = useState('');
  const mutation = useCreateWorkspace();

  useEffect(() => {
    if (open && !machineId && machines[0]) setMachineId(machines[0].id);
  }, [open, machineId, machines]);

  useEffect(() => {
    if (open) {
      setRootPath('');
      setName('');
    }
  }, [open]);

  const canSubmit = Boolean(machineId && rootPath.trim()) && !mutation.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[520px]">
        <DialogHeader
          title="Add workspace"
          description="Register a directory on a connected machine."
          onClose={() => onOpenChange(false)}
        />
        <form
          className="mt-5 flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            if (!canSubmit) return;
            mutation.mutate(
              {
                machineId,
                name: name.trim() || undefined,
                rootPath: rootPath.trim(),
              },
              { onSuccess: () => onOpenChange(false) },
            );
          }}
        >
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="add-workspace-machine"
              className="text-[11px] font-bold text-muted-foreground"
            >
              Machine
            </label>
            <Select
              id="add-workspace-machine"
              value={machineId}
              onValueChange={setMachineId}
              options={machines.map((machine) => ({
                value: machine.id,
                label: machineDisplayName(machine),
                description: `${machine.os} · ${machine.status}`,
              }))}
              placeholder="Select a machine"
              disabled={mutation.isPending || machines.length === 0}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="add-workspace-name"
              className="text-[11px] font-bold text-muted-foreground"
            >
              Name
            </label>
            <Input
              id="add-workspace-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Optional; defaults to the folder name"
              disabled={mutation.isPending}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="add-workspace-path"
              className="text-[11px] font-bold text-muted-foreground"
            >
              Root path
            </label>
            <Input
              id="add-workspace-path"
              value={rootPath}
              onChange={(event) => setRootPath(event.target.value)}
              placeholder="/home/you/code/project"
              className="font-mono text-[11.5px]"
              disabled={mutation.isPending}
              required
            />
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
            <Button type="submit" variant="primary" disabled={!canSubmit}>
              {mutation.isPending ? 'Adding…' : 'Add workspace'}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

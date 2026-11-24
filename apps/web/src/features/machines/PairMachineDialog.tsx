import type { PairingCode } from '@agent-fleet/protocol-types';
import { useMutation } from '@tanstack/react-query';
import { Check, Copy } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { apiClient } from '@/lib/api/client';
import { API_BASE_URL } from '@/lib/api/config';
import { formatDateTime, getErrorMessage } from '@/lib/utils/format';

/** WebSocket URL the daemon should use, derived from the API origin. */
function machineSocketUrl(): string {
  const base = API_BASE_URL.startsWith('http')
    ? API_BASE_URL.replace(/\/api\/v1$/, '')
    : window.location.origin;
  const url = new URL(base);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  const path = url.pathname.replace(/\/+$/, '');
  url.pathname = `${path}/ws/machine`;
  return url.toString();
}

/**
 * Create a single-use pairing code and show the command that redeems it.
 *
 * The code is held only in component state and is displayed once.
 */
export function PairMachineDialog({ orgId, onClose }: { orgId: string; onClose: () => void }) {
  const [name, setName] = useState('');
  const [created, setCreated] = useState<PairingCode | null>(null);
  const [copied, setCopied] = useState<'code' | 'command' | null>(null);
  const mutation = useMutation({
    mutationFn: () =>
      apiClient.createPairingCode({
        org_id: orgId || undefined,
        machine_name: name.trim() || undefined,
      }),
    onSuccess: setCreated,
  });

  const command = created
    ? `fleet daemon pair --server ${machineSocketUrl()} --code ${created.code}`
    : '';

  const copy = async (value: string, kind: 'code' | 'command') => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(kind);
    } catch {
      setCopied(null);
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <DialogContent className="max-w-[560px]">
        <DialogHeader
          title={created ? 'Pair a machine' : 'Create a pairing code'}
          description={
            created
              ? 'Run this command on the machine you want to enroll. The code works once and expires soon.'
              : 'The code joins a new daemon to the active organization. It is shown once.'
          }
          onClose={onClose}
        />
        {created ? (
          <div className="mt-4 flex flex-col gap-3">
            <div className="flex items-center gap-1.5">
              <code
                data-testid="pairing-code"
                className="min-w-0 flex-1 overflow-x-auto rounded-md border border-border bg-muted px-3 py-2.5 font-mono text-[14px] tracking-[0.08em] text-foreground"
              >
                {created.code}
              </code>
              <Button
                variant="secondary"
                size="icon"
                onClick={() => void copy(created.code, 'code')}
                aria-label="Copy pairing code"
              >
                {copied === 'code' ? <Check className="size-4" /> : <Copy className="size-4" />}
              </Button>
            </div>

            <div className="flex items-start gap-1.5">
              <code
                data-testid="pairing-command"
                className="min-w-0 flex-1 overflow-x-auto rounded-md border border-border bg-muted px-3 py-2 font-mono text-[11.5px] leading-5 text-foreground"
              >
                {command}
              </code>
              <Button
                variant="secondary"
                size="icon"
                onClick={() => void copy(command, 'command')}
                aria-label="Copy pairing command"
              >
                {copied === 'command' ? <Check className="size-4" /> : <Copy className="size-4" />}
              </Button>
            </div>

            <p className="text-[10.5px] leading-4 text-faint">
              Expires {formatDateTime(created.expires_at)}. The daemon stores its own machine token
              after redeeming the code; the code itself is not reusable.
            </p>

            <div className="mt-1 flex items-center justify-end">
              <Button variant="primary" onClick={onClose}>
                Done
              </Button>
            </div>
          </div>
        ) : (
          <form
            className="mt-5 flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (!mutation.isPending) mutation.mutate();
            }}
          >
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="pair-machine-name"
                className="text-[11px] font-bold text-muted-foreground"
              >
                Machine name (optional)
              </label>
              <Input
                id="pair-machine-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder="build-01"
                disabled={mutation.isPending}
              />
            </div>

            {mutation.error ? (
              <p className="rounded-md border border-destructive/20 bg-destructive-soft px-3 py-2 text-[11.5px] text-destructive">
                {getErrorMessage(mutation.error)}
              </p>
            ) : null}

            <div className="mt-1 flex items-center justify-end gap-2">
              <Button variant="secondary" onClick={onClose} disabled={mutation.isPending}>
                Cancel
              </Button>
              <Button type="submit" variant="primary" disabled={mutation.isPending}>
                {mutation.isPending ? 'Creating…' : 'Create code'}
              </Button>
            </div>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

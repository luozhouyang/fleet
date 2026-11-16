import type {
  AgentPermissionOption,
  AgentPermissionRequest,
} from '@agent-fleet/protocol-types';
import { FileText, ShieldAlert } from 'lucide-react';
import { useEffect, useMemo } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Kbd } from '@/components/ui/kbd';
import { permissionFilePaths, permissionPreview, permissionRisk } from './session-utils';

const OPTION_LABELS: Record<string, string> = {
  allow_once: 'Allow once',
  allow_always: 'Allow always',
  reject_once: 'Reject',
  reject_always: 'Always reject',
};

function optionIsApprove(option: AgentPermissionOption): boolean {
  return option.kind.startsWith('allow');
}

function optionLabel(option: AgentPermissionOption): string {
  return OPTION_LABELS[option.kind] ?? option.name;
}

export function ApprovalDialog({
  permission,
  open,
  onOpenChange,
  pending,
  error,
  options,
  onApprove,
  onReject,
  onResolve,
}: {
  permission: AgentPermissionRequest;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  pending: boolean;
  error?: string;
  options?: AgentPermissionOption[];
  onApprove: () => void;
  onReject: () => void;
  onResolve?: (optionId: string, approve: boolean) => void;
}) {
  const preview = permissionPreview(permission);
  const paths = permissionFilePaths(permission);
  const resolvedOptions = useMemo(() => options ?? [], [options]);
  const hasOptions = resolvedOptions.length > 0;

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (hasOptions) {
        if (event.key === 'Enter' && !event.shiftKey) {
          const option = resolvedOptions.find((item) => item.kind.startsWith('allow'));
          if (option && !pending && onResolve) {
            event.preventDefault();
            onResolve(option.option_id, true);
          }
          return;
        }
        if (event.key === 'Escape') {
          const option = resolvedOptions.find((item) => item.kind.startsWith('reject'));
          if (option && !pending && onResolve) {
            event.preventDefault();
            onResolve(option.option_id, false);
          }
          return;
        }
        return;
      }
      if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return;
      event.preventDefault();
      if (!pending) onApprove();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [open, pending, onApprove, hasOptions, resolvedOptions, onResolve]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader
          eyebrow={`Permission / ${permission.tool_name}`}
          title="Approve this action?"
          description="The agent is waiting on your decision before it continues."
          icon={<ShieldAlert className="size-5" />}
          iconClassName="bg-warning-soft text-warning"
          onClose={() => onOpenChange(false)}
        />

        <div className="mt-4 flex flex-col gap-3">
          <div>
            <p className="font-mono text-[9px] uppercase tracking-[0.12em] text-faint">
              {permission.tool_name}
            </p>
            <p className="mt-1 whitespace-pre-wrap break-words text-[12.5px] leading-5 text-body">
              {permission.description}
            </p>
          </div>

          {paths.length ? (
            <div>
              <p className="mb-1 font-mono text-[9px] uppercase tracking-[0.12em] text-faint">
                Files
              </p>
              <ul className="flex flex-col gap-1 rounded-md border border-border bg-inset px-3 py-2">
                {paths.map((path) => (
                  <li
                    key={path}
                    className="flex min-w-0 items-center gap-1.5 font-mono text-[11px] leading-5 text-body"
                  >
                    <FileText className="size-3 shrink-0 text-faint" aria-hidden="true" />
                    <span className="truncate">{path}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          {preview ? (
            <div>
              <p className="mb-1 font-mono text-[9px] uppercase tracking-[0.12em] text-faint">
                Command
              </p>
              <pre className="max-h-44 overflow-auto whitespace-pre-wrap break-all rounded-md border border-border bg-inset px-3 py-2 font-mono text-[11px] leading-5 text-body">
                {preview}
              </pre>
            </div>
          ) : null}

          <p className="rounded-md border border-warning/20 bg-warning-soft px-3 py-2 text-[11.5px] leading-5 text-warning">
            {permissionRisk(permission.tool_name)}
          </p>

          {error ? (
            <p className="rounded-md border border-destructive/20 bg-destructive-soft px-3 py-2 text-[11.5px] text-destructive">
              {error}
            </p>
          ) : null}
        </div>

        {hasOptions ? (
          <div className="mt-6 flex flex-col gap-2">
            <span className="hidden items-center gap-1 text-[10.5px] text-muted-foreground sm:flex">
              <Kbd>↵</Kbd>
              to allow ·
              <Kbd>Esc</Kbd>
              to reject
            </span>
            <div className="flex flex-wrap items-center justify-end gap-2">
              {resolvedOptions.map((option) => (
                <Button
                  key={option.option_id}
                  variant={optionIsApprove(option) ? 'primary' : 'secondary'}
                  disabled={pending}
                  onClick={() => onResolve?.(option.option_id, optionIsApprove(option))}
                >
                  {pending ? 'Working…' : optionLabel(option)}
                </Button>
              ))}
            </div>
          </div>
        ) : (
          <div className="mt-6 flex items-center justify-between gap-2">
            <span className="hidden items-center gap-1 text-[10.5px] text-muted-foreground sm:flex">
              <Kbd>⌘</Kbd>
              <Kbd>↵</Kbd>
              to approve
            </span>
            <div className="flex items-center gap-2">
              <Button variant="secondary" onClick={onReject} disabled={pending}>
                Reject
              </Button>
              <Button variant="primary" onClick={onApprove} disabled={pending}>
                {pending ? 'Working…' : 'Approve & continue'}
              </Button>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

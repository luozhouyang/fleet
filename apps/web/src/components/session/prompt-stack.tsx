import type { AgentPermissionRequest } from '@agent-fleet/protocol-types';
import { ChevronDown, ShieldAlert, TriangleAlert } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Kbd } from '@/components/ui/kbd';
import { canBulkApprove, remainingSeconds, resolveDecision } from '@/lib/session/prompt-stack';
import { cn } from '@/lib/utils/cn';
import { permissionPreview, permissionRisk } from './session-utils';

/**
 * The pending-permission stack above the composer.
 *
 * The oldest request is expanded; the rest queue behind it. Decision keys are
 * `a` to approve, `r` to reject and `j`/`k` to move between pending requests.
 * The shortcuts are scoped to the stack and ignore keystrokes typed into a
 * field, so composing a message never approves anything.
 */
export function PromptStack({
  permissions,
  sessionActive,
  busy,
  resolvingId,
  onResolve,
  onBulkApprove,
}: {
  permissions: AgentPermissionRequest[];
  sessionActive: boolean;
  busy: boolean;
  resolvingId: string | null;
  onResolve: (
    permission: AgentPermissionRequest,
    decision: 'approve' | 'reject',
    optionId?: string,
  ) => void;
  onBulkApprove: (permissions: AgentPermissionRequest[]) => void;
}) {
  const [expandedId, setExpandedId] = useState<string | null>(permissions[0]?.id ?? null);
  const [now, setNow] = useState(() => Date.now());
  // The deadline string is frozen when it is first announced: a live region
  // whose text changes every second would be read out on every tick.
  const [deadlineNotice, setDeadlineNotice] = useState<string | null>(null);

  useEffect(() => {
    if (permissions.length === 0) return;
    const handle = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(handle);
  }, [permissions.length]);

  useEffect(() => {
    if (!permissions.some((permission) => permission.id === expandedId)) {
      setExpandedId(permissions[0]?.id ?? null);
    }
  }, [permissions, expandedId]);

  const expanded = permissions.find((permission) => permission.id === expandedId) ?? permissions[0];
  const expiry = expanded ? remainingSeconds(expanded.expires_at, now) : null;

  // Announce the deadline once, when it crosses ten seconds. A new expanded
  // permission starts with a clean slate.
  useEffect(() => {
    setDeadlineNotice(null);
  }, [expandedId]);

  useEffect(() => {
    if (expiry === null || expiry > 10 || deadlineNotice !== null) return;
    setDeadlineNotice(`Permission expires in ${expiry} seconds`);
  }, [expiry, deadlineNotice]);

  useEffect(() => {
    if (!expanded) return;
    const handle = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)
      ) {
        return;
      }
      if (event.key === 'a' || event.key === 'r') {
        if (busy) return;
        event.preventDefault();
        const decision = resolveDecision(
          expanded.options,
          event.key === 'a' ? 'approve' : 'reject',
        );
        if (!decision) return;
        onResolve(expanded, event.key === 'a' ? 'approve' : 'reject', decision.optionId);
      }
    };
    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, [expanded, busy, onResolve]);

  if (permissions.length === 0 || !expanded) return null;

  const preview = permissionPreview(expanded);
  const bulk = canBulkApprove(permissions);
  const busyHere = busy && resolvingId === expanded.id;
  const canRespond = sessionActive && !busy;

  return (
    <section
      data-testid="prompt-stack"
      aria-label="Pending permissions"
      className="mb-2 overflow-hidden rounded-lg border border-warning/30 bg-warning-soft"
    >
      {/* One-shot assertive announcement for the deadline; a live timer
          would repeat the whole countdown every second. */}
      <span aria-live="assertive" className="sr-only">
        {deadlineNotice ?? ''}
      </span>
      <header className="flex items-center gap-2 px-3 pt-2">
        <TriangleAlert className="size-3.5 shrink-0 text-warning" aria-hidden="true" />
        <span className="text-[11.5px] font-bold text-foreground">
          {permissions.length === 1
            ? 'Permission required'
            : `${permissions.length} permissions waiting`}
        </span>
        <span className="ml-auto flex items-center gap-1 font-mono text-[9px] text-faint">
          {permissions.length > 1 ? (
            <span className="mr-1">
              {permissions.findIndex((permission) => permission.id === expanded.id) + 1} /{' '}
              {permissions.length}
            </span>
          ) : null}
          {expiry !== null ? (
            <span
              role="timer"
              aria-live="off"
              className={cn(expiry <= 10 && 'font-bold text-destructive')}
            >
              {expiry}s
            </span>
          ) : null}
        </span>
      </header>

      <div className="px-3 pb-2 pt-1">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="font-mono text-[10px] font-bold text-warning">{expanded.tool_name}</span>
          {sessionActive ? null : (
            <span className="text-[10px] text-muted-foreground">This session is closed.</span>
          )}
        </div>
        <p className="mt-1 whitespace-pre-wrap break-words text-[12px] leading-[18px] text-body">
          {expanded.description}
        </p>
        {preview ? (
          <pre className="mt-1.5 max-h-24 overflow-auto whitespace-pre-wrap break-all rounded-md border border-border bg-card px-2 py-1.5 font-mono text-[10px] leading-[15px] text-body">
            {preview}
          </pre>
        ) : null}
        <p className="mt-1.5 text-[10.5px] leading-4 text-warning">
          {permissionRisk(expanded.tool_name)}
        </p>

        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <Button
            variant="secondary"
            size="sm"
            disabled={!canRespond}
            onClick={() => onResolve(expanded, 'reject')}
          >
            Reject
          </Button>
          <Button
            variant="primary"
            size="sm"
            disabled={!canRespond}
            onClick={() => {
              const decision = resolveDecision(expanded.options, 'approve');
              if (decision) onResolve(expanded, 'approve', decision.optionId);
            }}
          >
            {busyHere ? 'Working…' : 'Approve'}
          </Button>
          {bulk ? (
            <Button
              variant="ghost"
              size="sm"
              disabled={!canRespond}
              title="Approve every waiting read"
              onClick={() => onBulkApprove(permissions)}
            >
              Approve all reads
            </Button>
          ) : null}
          <span className="ml-auto hidden items-center gap-1 font-mono text-[9px] text-faint sm:flex">
            <Kbd>a</Kbd> allow · <Kbd>r</Kbd> reject
          </span>
        </div>
      </div>

      {permissions.length > 1 ? (
        <ul className="border-t border-warning/20 bg-card/50">
          {permissions
            .filter((permission) => permission.id !== expanded.id)
            .map((permission) => (
              <li key={permission.id}>
                <button
                  type="button"
                  onClick={() => setExpandedId(permission.id)}
                  className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-muted/50"
                >
                  <ShieldAlert className="size-3 shrink-0 text-warning" aria-hidden="true" />
                  <span className="shrink-0 font-mono text-[10px] font-bold text-foreground">
                    {permission.tool_name}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-[10.5px] text-muted-foreground">
                    {permission.description}
                  </span>
                  <ChevronDown className="size-3 shrink-0 text-faint" aria-hidden="true" />
                </button>
              </li>
            ))}
        </ul>
      ) : null}
    </section>
  );
}

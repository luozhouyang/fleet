import {
  PermissionStatus,
  type AgentPermissionRequest,
} from '@agent-fleet/protocol-types';
import { Badge, Button, cn } from '@agent-fleet/ui';
import { useEffect, useState } from 'react';
import { PermissionStatusBadge } from '@/components/status/StatusBadge';
import { formatDateTime, formatJson } from '@/lib/utils/format';

export function PermissionCard({
  permission,
  sessionActive,
  resolving,
  onResolve,
}: {
  permission: AgentPermissionRequest;
  sessionActive: boolean;
  resolving: boolean;
  onResolve: (permission: AgentPermissionRequest, approve: boolean) => void;
}) {
  const [now, setNow] = useState(() => Date.now());
  const expiresAt = permission.expires_at ? Date.parse(permission.expires_at) : NaN;
  const isExpired =
    permission.status === PermissionStatus.Pending &&
    Number.isFinite(expiresAt) &&
    expiresAt <= now;
  const effectiveStatus = isExpired ? PermissionStatus.TimedOut : permission.status;
  const pending = effectiveStatus === PermissionStatus.Pending;
  const canRespond = pending && sessionActive && !resolving;

  useEffect(() => {
    if (permission.status !== PermissionStatus.Pending || !permission.expires_at) return;
    const expires = Date.parse(permission.expires_at);
    if (!Number.isFinite(expires)) return;
    const delay = Math.max(0, expires - Date.now()) + 50;
    const timer = window.setTimeout(() => setNow(Date.now()), delay);
    return () => window.clearTimeout(timer);
  }, [permission.status, permission.expires_at]);

  return (
    <div
      data-testid="permission-card"
      className={cn(
        'w-full max-w-[52rem] overflow-hidden rounded-lg border bg-slate-900/55',
        pending ? 'border-amber-500/30' : 'border-slate-800',
      )}
    >
      <div className="flex flex-wrap items-center gap-2 border-b border-slate-800/80 px-3 py-2.5">
        <span className="flex size-6 items-center justify-center rounded-md bg-amber-500/10 text-amber-300">
          <svg viewBox="0 0 20 20" fill="none" className="size-3.5" aria-hidden="true">
            <path
              d="M10 2.5 17.5 16h-15L10 2.5Z"
              stroke="currentColor"
              strokeWidth="1.4"
              strokeLinejoin="round"
            />
            <path d="M10 7v4m0 2.5v.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
          </svg>
        </span>
        <span className="text-xs font-semibold text-slate-200">Permission required</span>
        <span className="font-mono text-[11px] text-slate-400">{permission.tool_name}</span>
        <PermissionStatusBadge status={effectiveStatus} />
      </div>

      <div className="space-y-3 p-3">
        <p className="text-xs leading-5 text-slate-300">{permission.description}</p>

        <details className="rounded-md border border-slate-800 bg-slate-950/40">
          <summary className="cursor-pointer list-none px-3 py-2 text-[11px] text-slate-500 [&::-webkit-details-marker]:hidden">
            Show tool parameters
          </summary>
          <pre className="max-h-56 overflow-auto border-t border-slate-800 p-3 font-mono text-[11px] leading-5 text-slate-400">
            {formatJson(permission.input)}
          </pre>
        </details>

        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-slate-600">
          <span>Requested {formatDateTime(permission.created_at)}</span>
          {permission.expires_at ? (
            <span>Expires {formatDateTime(permission.expires_at)}</span>
          ) : null}
          {permission.resolved_at ? (
            <span>Resolved {formatDateTime(permission.resolved_at)}</span>
          ) : null}
        </div>

        {pending ? (
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <p className="text-[11px] leading-5 text-slate-500">
              {!sessionActive
                ? 'This session is no longer accepting permission decisions.'
                : 'Approving allows the agent to perform this action.'}
            </p>
            <div className="flex shrink-0 gap-2">
              <Button
                variant="danger"
                size="sm"
                disabled={!canRespond}
                loading={resolving}
                onClick={() => onResolve(permission, false)}
              >
                Reject
              </Button>
              <Button
                variant="primary"
                size="sm"
                disabled={!canRespond}
                loading={resolving}
                onClick={() => onResolve(permission, true)}
              >
                Approve
              </Button>
            </div>
          </div>
        ) : (
          <div className="flex items-center gap-2 text-[11px] text-slate-500">
            <Badge tone={effectiveStatus === PermissionStatus.Approved ? 'green' : 'slate'}>
              {effectiveStatus === PermissionStatus.Approved
                ? 'Approved'
                : effectiveStatus === PermissionStatus.Rejected
                  ? 'Rejected'
                  : effectiveStatus === PermissionStatus.TimedOut
                    ? 'Expired before a decision'
                    : effectiveStatus === PermissionStatus.Cancelled
                      ? 'Cancelled'
                      : 'Decision could not be delivered'}
            </Badge>
            {isExpired ? <span>No response can be sent.</span> : null}
          </div>
        )}
      </div>
    </div>
  );
}

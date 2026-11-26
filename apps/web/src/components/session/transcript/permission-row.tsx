import { PermissionStatus, type AgentPermissionRequest } from '@agent-fleet/protocol-types';
import { TriangleAlert } from 'lucide-react';
import { memo } from 'react';
import { Button } from '@/components/ui/button';
import { StatusDot } from '@/components/ui/status-dot';
import { useSessionStore } from '@/lib/session-store';
import { formatRelativeTime } from '@/lib/utils/format';
import { permissionPreview, permissionStatusMeta } from '../session-utils';

/**
 * A permission request inline in the transcript.
 *
 * The row subscribes to its own permission so an approval from anywhere
 * updates this card without re-rendering the rest of the transcript.
 */
export const PermissionRow = memo(function PermissionRow({
  sessionId,
  permissionId,
  sessionActive,
  busy,
  resolving,
  onApprove,
  onReject,
}: {
  sessionId: string;
  permissionId: string;
  sessionActive: boolean;
  busy: boolean;
  resolving: boolean;
  onApprove: (permission: AgentPermissionRequest) => void;
  onReject: (permission: AgentPermissionRequest) => void;
}) {
  const permission = useSessionStore(
    (state) => state.sessions[sessionId]?.permissions[permissionId],
  );
  if (!permission) return null;

  const status = permissionStatusMeta(permission.status);
  const pending = permission.status === PermissionStatus.Pending;
  const preview = permissionPreview(permission);
  const canRespond = pending && sessionActive && !busy;

  return (
    <div
      role={pending ? 'alert' : undefined}
      className="mb-4 rounded-lg bg-warning-soft p-3"
      data-testid="permission-card"
      data-permission-id={permission.id}
    >
      <div className="flex flex-wrap items-center gap-2">
        <TriangleAlert className="size-3.5 shrink-0 text-warning" aria-hidden="true" />
        <span className="text-[12.5px] font-bold text-foreground">Permission required</span>
        <span className="font-mono text-[10px] text-warning">{permission.tool_name}</span>
      </div>

      <p className="mt-1.5 whitespace-pre-wrap break-words text-[12px] leading-[18px] text-body">
        {permission.description}
      </p>

      {preview ? (
        <pre className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap break-all rounded-md border border-border bg-card px-2.5 py-2 font-mono text-[10.5px] leading-[16px] text-body">
          {preview}
        </pre>
      ) : null}

      {pending ? (
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button
            variant="secondary"
            size="sm"
            disabled={!canRespond}
            onClick={() => onReject(permission)}
          >
            Reject
          </Button>
          <Button
            variant="primary"
            size="sm"
            disabled={!canRespond}
            onClick={() => onApprove(permission)}
          >
            {resolving ? 'Working…' : 'Approve'}
          </Button>
          {!sessionActive ? (
            <span className="text-[10.5px] text-muted-foreground">This session is closed.</span>
          ) : null}
        </div>
      ) : (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <StatusDot tone={status.tone} size="sm" label={status.label} />
          {permission.resolved_at ? (
            <span className="font-mono text-[9px] text-faint">
              resolved {formatRelativeTime(permission.resolved_at)}
            </span>
          ) : null}
        </div>
      )}
    </div>
  );
});

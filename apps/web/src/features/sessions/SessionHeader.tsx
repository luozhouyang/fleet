import {
  SessionStatus,
  type AgentSession,
  type ApiError,
} from '@agent-fleet/protocol-types';
import { Badge, Button, Card } from '@agent-fleet/ui';
import { Link } from '@tanstack/react-router';
import type { ReactNode } from 'react';
import { SessionStatusBadge } from '@/components/status/StatusBadge';
import { formatDateTime, shortId } from '@/lib/utils/format';

const interruptible = new Set([
  SessionStatus.Starting,
  SessionStatus.Running,
  SessionStatus.WaitingForInput,
  SessionStatus.WaitingForPermission,
]);

const resumable = new Set([SessionStatus.Interrupted, SessionStatus.Failed]);

export function SessionHeader({
  session,
  status,
  lastError,
  pendingAction,
  onInterrupt,
  onResume,
  onClose,
}: {
  session: AgentSession;
  status: SessionStatus;
  lastError?: ApiError;
  pendingAction: 'interrupt' | 'resume' | 'close' | null;
  onInterrupt: () => void;
  onResume: () => void;
  onClose: () => void;
}) {
  return (
    <Card className="shrink-0">
      <div className="flex flex-col gap-4 p-4">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h1 className="truncate text-base font-semibold text-slate-50">
                {session.title || `Session ${shortId(session.id, 10)}`}
              </h1>
              <SessionStatusBadge status={status} />
            </div>
            <p className="mt-1 text-[11px] text-slate-500">
              Created {formatDateTime(session.created_at)}
            </p>
          </div>

          <div className="flex shrink-0 flex-wrap items-center gap-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={onInterrupt}
              disabled={!interruptible.has(status) || pendingAction !== null}
              loading={pendingAction === 'interrupt'}
            >
              Interrupt
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={onResume}
              disabled={!resumable.has(status) || pendingAction !== null}
              loading={pendingAction === 'resume'}
            >
              Resume
            </Button>
            <Button
              variant="danger"
              size="sm"
              onClick={onClose}
              disabled={status === SessionStatus.Terminated || pendingAction !== null}
              loading={pendingAction === 'close'}
            >
              Close
            </Button>
          </div>
        </div>

        <div className="grid gap-2 sm:grid-cols-3">
          <Meta label="Provider">
            <span className="font-mono text-xs text-slate-300">{session.provider}</span>
            <Badge tone="slate">{session.agent_type}</Badge>
          </Meta>
          <Meta label="Machine">
            <Link
              to="/machines/$machineId"
              params={{ machineId: session.machine_id }}
              className="truncate text-xs text-teal-300 hover:text-teal-200"
            >
              {shortId(session.machine_id, 14)}
            </Link>
          </Meta>
          <Meta label="Workspace">
            {session.workspace_id ? (
              <Link
                to="/workspaces/$workspaceId"
                params={{ workspaceId: session.workspace_id }}
                className="truncate text-xs text-teal-300 hover:text-teal-200"
              >
                {shortId(session.workspace_id, 14)}
              </Link>
            ) : (
              <span className="text-xs text-slate-500">Not attached</span>
            )}
          </Meta>
        </div>

        <div className="flex min-w-0 items-center gap-2 rounded-md border border-slate-800 bg-slate-950/45 px-2.5 py-1.5">
          <span className="text-[10px] uppercase tracking-wider text-slate-600">cwd</span>
          <span className="truncate font-mono text-[11px] text-slate-400" title={session.cwd}>
            {session.cwd}
          </span>
        </div>

        {lastError ? (
          <div className="rounded-md border border-red-500/25 bg-red-500/5 px-3 py-2 text-xs leading-5 text-red-300">
            <span className="font-mono text-[10px] uppercase tracking-wider text-red-400">
              {lastError.code}
            </span>
            <span className="mx-2 text-red-500/40">·</span>
            {lastError.message}
          </div>
        ) : null}
      </div>
    </Card>
  );
}

function Meta({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 items-center gap-2 rounded-md border border-slate-800/70 bg-slate-950/30 px-2.5 py-1.5">
      <span className="shrink-0 text-[10px] uppercase tracking-wider text-slate-600">
        {label}
      </span>
      <span className="flex min-w-0 items-center gap-1.5 truncate">{children}</span>
    </div>
  );
}

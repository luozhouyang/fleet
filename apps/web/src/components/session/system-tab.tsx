import {
  PermissionStatus,
  type AgentPermissionRequest,
  type AgentSession,
  type SessionDetail,
  type SessionStatus,
} from '@agent-fleet/protocol-types';
import type { ReactNode } from 'react';
import { StatusDot } from '@/components/ui/status-dot';
import type { SessionViewState } from '@/lib/session-reducer';
import { sessionStatusMeta } from '@/lib/status';
import { formatDateTime, formatRelativeTime, shortId } from '@/lib/utils/format';
import { permissionStatusMeta, sessionIsRestored, sessionModel } from './session-utils';

export function SystemTab({
  session,
  status,
  view,
  detail,
  machineName,
  workspaceName,
}: {
  session: AgentSession;
  status: SessionStatus;
  view?: SessionViewState;
  detail?: SessionDetail;
  machineName?: string;
  workspaceName?: string;
}) {
  const statusMeta = sessionStatusMeta(status);
  const model = sessionModel(session);
  const turnCount = view?.turnOrder.length || detail?.turns.length || 0;
  const messageCount = view?.messageOrder.length || detail?.messages.length || 0;
  const toolCount = view?.toolOrder.length ?? 0;
  const permissions =
    view && view.permissionOrder.length
      ? view.permissionOrder
          .map((id) => view.permissions[id])
          .filter((permission): permission is AgentPermissionRequest => Boolean(permission))
      : (detail?.permissions ?? []);
  const pendingPermissions = permissions.filter(
    (permission) => permission.status === PermissionStatus.Pending,
  ).length;
  const lastError = view?.lastError ?? session.last_error;
  const restored = sessionIsRestored(session);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-5 pt-2">
      <SectionLabel>Session</SectionLabel>
      <InfoRow label="Session">
        <span className="break-all" title={session.id}>
          {session.id}
        </span>
      </InfoRow>
      <InfoRow label="Provider">{session.provider}</InfoRow>
      <InfoRow label="Agent">{session.agent_type}</InfoRow>
      {model ? <InfoRow label="Model">{model}</InfoRow> : null}
      <InfoRow label="CWD">
        <span className="break-all" title={session.cwd}>
          {session.cwd || '—'}
        </span>
      </InfoRow>
      <InfoRow label="Status">
        <StatusDot tone={statusMeta.tone} size="sm" label={statusMeta.label} />
      </InfoRow>
      <InfoRow label="Created">
        <span title={formatDateTime(session.created_at)}>
          {formatRelativeTime(session.created_at)}
        </span>
      </InfoRow>
      <InfoRow label="Updated">
        <span title={formatDateTime(session.updated_at)}>
          {formatRelativeTime(session.updated_at)}
        </span>
      </InfoRow>
      {restored ? (
        <InfoRow label="History">Restored after restart (read-only)</InfoRow>
      ) : null}

      <SectionLabel>Workspace</SectionLabel>
      <InfoRow label="Machine">{machineName ?? 'Unknown machine'}</InfoRow>
      <InfoRow label="Workspace">
        {session.workspace_id
          ? (workspaceName ?? shortId(session.workspace_id, 14))
          : 'Not attached'}
      </InfoRow>

      <SectionLabel>Activity</SectionLabel>
      <InfoRow label="Turns">{turnCount}</InfoRow>
      <InfoRow label="Messages">{messageCount}</InfoRow>
      <InfoRow label="Tools">{toolCount}</InfoRow>
      <InfoRow label="Pending">{pendingPermissions}</InfoRow>

      <SectionLabel>Last error</SectionLabel>
      {lastError ? (
        <div className="rounded-md border border-destructive/20 bg-destructive-soft px-2.5 py-2">
          <p className="font-mono text-[9px] uppercase tracking-[0.1em] text-destructive">
            {lastError.code}
          </p>
          <p className="mt-1 whitespace-pre-wrap break-words text-[10.5px] leading-4 text-destructive">
            {lastError.message}
          </p>
        </div>
      ) : (
        <p className="text-[11px] text-faint">No errors recorded.</p>
      )}

      <SectionLabel>Permissions</SectionLabel>
      {permissions.length === 0 ? (
        <p className="text-[11px] text-faint">No permission requests recorded.</p>
      ) : (
        <div className="flex flex-col">
          {permissions.map((permission) => {
            const meta = permissionStatusMeta(permission.status);
            return (
              <div
                key={permission.id}
                className="flex items-start justify-between gap-3 border-b border-border py-2 last:border-0"
              >
                <div className="min-w-0">
                  <p className="truncate font-mono text-[10.5px] text-body">
                    {permission.tool_name}
                  </p>
                  <p className="mt-0.5 line-clamp-2 text-[10.5px] leading-4 text-muted-foreground">
                    {permission.description}
                  </p>
                </div>
                <StatusDot tone={meta.tone} size="sm" label={meta.label} className="shrink-0" />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <p className="mb-1 mt-4 border-b border-border pb-1 font-mono text-[9px] uppercase tracking-[0.12em] text-faint first:mt-0">
      {children}
    </p>
  );
}

function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[80px_minmax(0,1fr)] gap-3 py-1.5">
      <span className="font-mono text-[9px] uppercase leading-4 tracking-[0.1em] text-faint">
        {label}
      </span>
      <span className="min-w-0 break-words font-mono text-[10.5px] leading-4 text-body">
        {children}
      </span>
    </div>
  );
}

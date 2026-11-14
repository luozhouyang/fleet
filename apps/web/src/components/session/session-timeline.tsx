import {
  MessageRole,
  PermissionStatus,
  type AgentMessage,
  type AgentPermissionRequest,
} from '@agent-fleet/protocol-types';
import { MessageSquare, TriangleAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { EmptyState } from '@/components/ui/feedback';
import { StatusDot } from '@/components/ui/status-dot';
import type { SessionToolCall } from '@/lib/session-reducer';
import type { TimelineEntry } from '@/lib/session-timeline';
import { formatClock, formatDuration, formatRelativeTime } from '@/lib/utils/format';
import {
  compactJson,
  permissionPreview,
  permissionStatusMeta,
  prettyJson,
  toolStatusMeta,
} from './session-utils';

function actorForEntry(entry: TimelineEntry): string {
  if (entry.kind === 'message') {
    if (entry.message.role === MessageRole.User) return 'You';
    if (entry.message.role === MessageRole.Assistant) return 'Agent';
    return 'System';
  }
  if (entry.kind === 'tool') return 'Tool';
  return 'Permission';
}

function toolDuration(tool: SessionToolCall): string | null {
  if (!tool.completed_at) return null;
  const started = Date.parse(tool.started_at);
  const completed = Date.parse(tool.completed_at);
  if (Number.isNaN(started) || Number.isNaN(completed) || completed < started) return null;
  return formatDuration((completed - started) / 1000);
}

export function SessionTimeline({
  entries,
  streaming,
  sessionActive,
  permissionBusy,
  resolvingPermissionId,
  onApprovePermission,
  onRejectPermission,
}: {
  entries: TimelineEntry[];
  streaming: boolean;
  sessionActive: boolean;
  permissionBusy: boolean;
  resolvingPermissionId: string | null;
  onApprovePermission: (permission: AgentPermissionRequest) => void;
  onRejectPermission: (permission: AgentPermissionRequest) => void;
}) {
  if (entries.length === 0) {
    return (
      <EmptyState
        icon={<MessageSquare className="size-4" />}
        title="No messages yet"
        description="Send the first instruction below. Streaming responses, tool calls and permission requests will appear here."
      />
    );
  }

  const lastEntry = entries[entries.length - 1];
  const streamingMessageId =
    streaming && lastEntry?.kind === 'message' && lastEntry.message.role === MessageRole.Assistant
      ? lastEntry.id
      : null;

  return (
    <div role="log" aria-live="polite" aria-relevant="additions text" className="flex flex-col">
      {entries.map((entry) => (
        <div key={`${entry.kind}-${entry.id}`} className="flex gap-3">
          <div className="w-[42px] shrink-0 pt-0.5 text-right">
            <div className="font-mono text-[9px] leading-3 text-faint">{formatClock(entry.at)}</div>
            <div className="mt-1 font-mono text-[8px] font-bold uppercase leading-3 tracking-[0.08em] text-primary">
              {actorForEntry(entry)}
            </div>
          </div>
          <div className="min-w-0 flex-1 pb-5">
            {entry.kind === 'message' ? (
              <MessageEntry message={entry.message} streaming={streamingMessageId === entry.id} />
            ) : entry.kind === 'tool' ? (
              <ToolCard tool={entry.tool} />
            ) : (
              <PermissionCard
                permission={entry.permission}
                sessionActive={sessionActive}
                busy={permissionBusy}
                resolving={permissionBusy && resolvingPermissionId === entry.permission.id}
                onApprove={onApprovePermission}
                onReject={onRejectPermission}
              />
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

function MessageEntry({ message, streaming }: { message: AgentMessage; streaming: boolean }) {
  if (message.role === MessageRole.User) {
    return (
      <div className="max-w-[600px] whitespace-pre-wrap break-words rounded-lg bg-primary-soft p-3 text-[12.5px] leading-[19px] text-foreground">
        {message.content}
      </div>
    );
  }
  if (message.role === MessageRole.System) {
    return (
      <p className="whitespace-pre-wrap break-words text-[11.5px] leading-[19px] text-muted-foreground">
        {message.content}
      </p>
    );
  }
  return (
    <p className="max-w-[680px] whitespace-pre-wrap break-words text-[12.5px] leading-[19px] text-body">
      {message.content}
      {streaming ? (
        <span
          className="ml-0.5 inline-block h-[13px] w-[5px] translate-y-[2px] animate-pulse rounded-sm bg-primary"
          aria-hidden="true"
        />
      ) : null}
    </p>
  );
}

function ToolCard({ tool }: { tool: SessionToolCall }) {
  const status = toolStatusMeta(tool.status);
  const input = tool.inputDraft ?? compactJson(tool.input, 600);
  const output = tool.output === undefined ? null : compactJson(tool.output, 800);
  const fullInput = prettyJson(tool.input);
  const fullOutput = prettyJson(tool.output);
  const duration = toolDuration(tool);

  return (
    <div className="max-w-[600px] rounded-lg border border-border bg-card p-3 shadow-card-sm">
      <div className="flex flex-wrap items-center gap-2">
        <span className="min-w-0 truncate font-mono text-[11px] font-bold text-foreground">
          {tool.name}
        </span>
        <StatusDot tone={status.tone} size="sm" label={status.label} />
        {duration ? (
          <span className="ml-auto shrink-0 font-mono text-[9px] text-faint">{duration}</span>
        ) : null}
      </div>

      {input ? (
        <pre className="mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-all rounded-md bg-inset px-2 py-1.5 font-mono text-[10px] leading-[15px] text-body">
          {input}
        </pre>
      ) : null}

      {output ? (
        <pre className="mt-1.5 max-h-40 overflow-auto whitespace-pre-wrap break-all rounded-md bg-inset px-2 py-1.5 font-mono text-[10px] leading-[15px] text-muted-foreground">
          {output}
        </pre>
      ) : null}

      {fullInput || fullOutput ? (
        <details className="mt-2 group">
          <summary className="cursor-pointer select-none font-mono text-[9.5px] text-muted-foreground hover:text-foreground">
            Full payload
          </summary>
          {fullInput ? (
            <pre className="mt-1.5 max-h-56 overflow-auto whitespace-pre-wrap break-all rounded-md bg-inset px-2 py-1.5 font-mono text-[10px] leading-[15px] text-body">
              {fullInput}
            </pre>
          ) : null}
          {fullOutput ? (
            <pre className="mt-1.5 max-h-56 overflow-auto whitespace-pre-wrap break-all rounded-md bg-inset px-2 py-1.5 font-mono text-[10px] leading-[15px] text-muted-foreground">
              {fullOutput}
            </pre>
          ) : null}
        </details>
      ) : null}

      {tool.error ? (
        <p className="mt-2 whitespace-pre-wrap break-words rounded-md bg-destructive-soft px-2 py-1.5 text-[10.5px] leading-4 text-destructive">
          {tool.error}
        </p>
      ) : null}
    </div>
  );
}

function PermissionCard({
  permission,
  sessionActive,
  busy,
  resolving,
  onApprove,
  onReject,
}: {
  permission: AgentPermissionRequest;
  sessionActive: boolean;
  busy: boolean;
  resolving: boolean;
  onApprove: (permission: AgentPermissionRequest) => void;
  onReject: (permission: AgentPermissionRequest) => void;
}) {
  const status = permissionStatusMeta(permission.status);
  const pending = permission.status === PermissionStatus.Pending;
  const preview = permissionPreview(permission);
  const canRespond = pending && sessionActive && !busy;

  return (
    <div
      role={pending ? 'alert' : undefined}
      className="max-w-[600px] rounded-lg bg-warning-soft p-3"
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
}

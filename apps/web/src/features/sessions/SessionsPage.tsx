import type { AgentSession } from '@agent-fleet/protocol-types';
import { Badge, Card, EmptyState, PageHeader, Spinner } from '@agent-fleet/ui';
import { Link } from '@tanstack/react-router';
import { ErrorState } from '@/components/feedback/ErrorState';
import { SessionStatusBadge } from '@/components/status/StatusBadge';
import { useMachines, useSessionList } from '@/lib/query/hooks';
import { formatDateTime, formatRelativeTime, shortId } from '@/lib/utils/format';

export function SessionsPage() {
  const sessionsQuery = useSessionList();
  const machinesQuery = useMachines();

  const sessions = [...(sessionsQuery.data ?? [])].sort(
    (left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at),
  );

  const machineNames = new Map(
    (machinesQuery.data ?? []).map((machine) => [machine.id, machine.name]),
  );
  const workspaceNames = new Map(
    (machinesQuery.data ?? []).flatMap((machine) =>
      (machine.workspaces ?? []).map((workspace) => [workspace.id, workspace.name] as const),
    ),
  );

  return (
    <div className="space-y-6">
      <PageHeader
        eyebrow="Agents"
        title="Sessions"
        description="Live and historical coding-agent sessions across every registered machine."
        actions={
          <>
            <span className="hidden items-center gap-2 text-xs text-slate-500 sm:flex">
              {sessionsQuery.isFetching ? (
                <Spinner size="sm" label="Refreshing sessions" />
              ) : (
                <span className="size-1.5 rounded-full bg-slate-600" />
              )}
              Auto-refresh 15s
            </span>
            <Link
              to="/machines"
              className="inline-flex h-9 items-center justify-center rounded-md border border-slate-700 bg-slate-900 px-4 text-sm font-medium text-slate-200 transition-colors hover:border-slate-600 hover:bg-slate-800 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60"
            >
              Start from a machine
            </Link>
          </>
        }
      />

      {sessionsQuery.isError ? (
        <ErrorState
          error={sessionsQuery.error}
          title="Could not load sessions"
          onRetry={() => void sessionsQuery.refetch()}
        />
      ) : sessionsQuery.isLoading ? (
        <Card className="flex min-h-56 items-center justify-center">
          <div className="flex items-center gap-3 text-sm text-slate-400">
            <Spinner size="sm" label="Loading sessions" />
            Loading sessions…
          </div>
        </Card>
      ) : sessions.length === 0 ? (
        <Card>
          <EmptyState
            title="No sessions yet"
            description="Open a machine with an available provider and start a session. Active sessions stream messages, tool calls and permission requests here."
            action={
              <Link
                to="/machines"
                className="inline-flex h-8 items-center justify-center rounded-md border border-slate-700 bg-slate-900 px-3 text-xs font-medium text-slate-200 transition-colors hover:border-slate-600 hover:bg-slate-800"
              >
                Browse machines
              </Link>
            }
          />
        </Card>
      ) : (
        <Card className="overflow-hidden">
          <div className="hidden grid-cols-[minmax(0,1.8fr)_minmax(0,1fr)_minmax(0,1fr)_150px_70px] gap-4 border-b border-slate-800/80 bg-slate-950/30 px-4 py-2.5 text-[10px] font-semibold uppercase tracking-[0.14em] text-slate-600 lg:grid">
            <span>Session</span>
            <span>Provider</span>
            <span>Location</span>
            <span>Created</span>
            <span />
          </div>
          <div className="divide-y divide-slate-800/70">
            {sessions.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                machineName={machineNames.get(session.machine_id)}
                workspaceName={
                  session.workspace_id ? workspaceNames.get(session.workspace_id) : undefined
                }
              />
            ))}
          </div>
        </Card>
      )}
    </div>
  );
}

function SessionRow({
  session,
  machineName,
  workspaceName,
}: {
  session: AgentSession;
  machineName: string | undefined;
  workspaceName: string | undefined;
}) {
  const sessionLink = (
    <Link
      to="/sessions/$sessionId"
      params={{ sessionId: session.id }}
      className="block min-w-0 group"
    >
      <span className="block truncate text-sm font-medium text-slate-100 group-hover:text-teal-200">
        {session.title || `Session ${shortId(session.id, 10)}`}
      </span>
      <span className="mt-0.5 block truncate font-mono text-[10px] text-slate-600">
        {session.id}
      </span>
    </Link>
  );

  return (
    <div className="grid gap-3 p-4 transition-colors hover:bg-slate-900/45 lg:grid-cols-[minmax(0,1.8fr)_minmax(0,1fr)_minmax(0,1fr)_150px_70px] lg:items-center lg:gap-4">
      <div className="flex min-w-0 items-start justify-between gap-3">
        {sessionLink}
        <span className="shrink-0">
          <SessionStatusBadge status={session.status} />
        </span>
      </div>

      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Badge tone="neutral" className="font-mono">
          {session.provider}
        </Badge>
        <span className="hidden text-[10px] text-slate-600 sm:inline">
          {session.agent_type}
        </span>
      </div>

      <div className="min-w-0 text-xs text-slate-400">
        <div className="truncate">
          {session.workspace_id ? (
            <Link
              to="/workspaces/$workspaceId"
              params={{ workspaceId: session.workspace_id }}
              className="hover:text-teal-300"
            >
              {workspaceName || shortId(session.workspace_id, 10)}
            </Link>
          ) : (
            <span className="text-slate-600">No workspace</span>
          )}
        </div>
        <div className="mt-0.5 truncate text-[10px] text-slate-600">
          {machineName || shortId(session.machine_id, 12)}
          <span className="mx-1.5 text-slate-700">·</span>
          updated {formatRelativeTime(session.updated_at)}
        </div>
      </div>

      <div className="text-xs text-slate-400">
        <div>{formatDateTime(session.created_at)}</div>
      </div>

      <div className="hidden justify-end lg:flex">
        <Link
          to="/sessions/$sessionId"
          params={{ sessionId: session.id }}
          className="inline-flex h-8 items-center justify-center rounded-md border border-slate-700 bg-slate-900 px-3 text-xs font-medium text-slate-200 transition-colors hover:border-slate-600 hover:bg-slate-800"
        >
          Open
        </Link>
      </div>
    </div>
  );
}

import type { AgentSession, Machine } from '@agent-fleet/protocol-types';
import { useNavigate } from '@tanstack/react-router';
import { GitBranch, Plus } from 'lucide-react';
import { useMemo, useState } from 'react';
import { AddWorkspaceDialog } from '@/components/dialogs/add-workspace-dialog';
import { NewSessionDialog } from '@/components/dialogs/new-session-dialog';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { StatusDot } from '@/components/ui/status-dot';
import {
  useGitStatus,
  useSessionList,
  useWorkspaceIndex,
  type WorkspaceLocation,
} from '@/lib/query/hooks';
import { machineStatusMeta, sessionIsActive } from '@/lib/status';
import { formatRelativeTime, getErrorMessage } from '@/lib/utils/format';
import { latestSessionActivity, workspaceSessionSummary } from './workspace-utils';

const LEDGER_GRID =
  'grid min-w-[900px] grid-cols-[minmax(0,1fr)_180px_120px_150px_130px] items-center gap-4';

interface WorkspaceGroup {
  machine: Machine;
  locations: WorkspaceLocation[];
}

const EMPTY_SESSIONS: AgentSession[] = [];

function plural(count: number, singular: string, pluralForm?: string): string {
  return `${count} ${count === 1 ? singular : (pluralForm ?? `${singular}s`)}`;
}

function WorkspaceRow({
  location,
  sessions,
  onOpen,
}: {
  location: WorkspaceLocation;
  sessions: AgentSession[];
  onOpen: () => void;
}) {
  const { workspace } = location;
  const gitStatus = useGitStatus(workspace.id, workspace.is_git_repository);
  const summary = workspaceSessionSummary(sessions);
  const activity = latestSessionActivity(sessions);
  const changed = new Set(gitStatus.data?.files?.map((file) => file.path) ?? []).size;

  return (
    <button
      type="button"
      onClick={onOpen}
      data-testid="workspace-row"
      data-id={workspace.id}
      className={`${LEDGER_GRID} w-full border-b border-border px-5 py-3 text-left transition-colors last:border-b-0 hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/35`}
    >
      <span className="flex min-w-0 flex-col">
        <span className="truncate text-[13px] font-bold leading-5 text-foreground">
          {workspace.name}
        </span>
        <span className="truncate font-mono text-[9px] leading-4 text-faint">
          {workspace.root_path}
        </span>
      </span>

      <span className="flex min-w-0 items-center gap-1.5">
        {workspace.is_git_repository ? (
          <>
            <GitBranch className="size-3 shrink-0 text-faint" aria-hidden="true" />
            <span className="truncate font-mono text-[10px] text-body">
              {gitStatus.data?.branch ?? (gitStatus.isLoading ? '…' : '—')}
            </span>
          </>
        ) : (
          <span className="font-mono text-[10px] text-faint">—</span>
        )}
      </span>

      <span className="font-mono text-[10px] text-body">
        {workspace.is_git_repository
          ? gitStatus.isLoading
            ? '…'
            : `${changed ?? 0} changed`
          : '—'}
      </span>

      <span className="flex min-w-0 items-center">
        <StatusDot tone={summary.tone} label={summary.label} />
      </span>

      <span className="truncate font-mono text-[9.5px] text-muted-foreground">
        {activity ? formatRelativeTime(activity) : '—'}
      </span>
    </button>
  );
}

export function WorkspacesPage() {
  const index = useWorkspaceIndex();
  const sessionsQuery = useSessionList();
  const navigate = useNavigate();
  const [addWorkspaceOpen, setAddWorkspaceOpen] = useState(false);
  const [newSessionOpen, setNewSessionOpen] = useState(false);

  const machines = index.data ?? [];
  const locations = index.locations;
  const sessions = sessionsQuery.data ?? EMPTY_SESSIONS;

  const groups = useMemo<WorkspaceGroup[]>(() => {
    const byMachine = new Map<string, WorkspaceGroup>();
    for (const location of locations) {
      const existing = byMachine.get(location.machine.id);
      if (existing) {
        existing.locations.push(location);
      } else {
        byMachine.set(location.machine.id, {
          machine: location.machine,
          locations: [location],
        });
      }
    }
    return [...byMachine.values()];
  }, [locations]);

  const sessionsByWorkspace = useMemo(() => {
    const byWorkspace = new Map<string, AgentSession[]>();
    for (const session of sessions) {
      if (!session.workspace_id) continue;
      const list = byWorkspace.get(session.workspace_id);
      if (list) {
        list.push(session);
      } else {
        byWorkspace.set(session.workspace_id, [session]);
      }
    }
    return byWorkspace;
  }, [sessions]);

  const busy = sessions.filter((session) => sessionIsActive(session.status)).length;
  const machineCount = new Set(locations.map((location) => location.machine.id)).size;

  const openWorkspace = (workspaceId: string) => {
    void navigate({ to: '/workspaces/$workspaceId', params: { workspaceId } });
  };

  return (
    <div className="h-full overflow-y-auto px-10 pb-10 pt-7">
      <div className="mx-auto flex max-w-[1400px] flex-col">
        <header className="flex flex-wrap items-end justify-between gap-6">
          <div className="min-w-0">
            <p className="font-mono text-[9px] font-medium uppercase tracking-[0.16em] text-faint">
              FLEET / WORKSPACES
            </p>
            <h1 className="mt-1 font-display text-[30px] font-bold leading-9 tracking-[-0.015em] text-foreground">
              Workspaces
            </h1>
            <p className="mt-1 text-[12.5px] leading-5 text-muted-foreground">
              {plural(locations.length, 'repository', 'repositories')} across{' '}
              {plural(machineCount, 'machine')}
              {' · '}
              {busy} busy
            </p>
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <Button
              variant="secondary"
              onClick={() => setAddWorkspaceOpen(true)}
              disabled={machines.length === 0}
            >
              <Plus className="size-3.5" aria-hidden="true" />
              Add workspace
            </Button>
            <Button
              variant="primary"
              onClick={() => setNewSessionOpen(true)}
              disabled={machines.length === 0}
            >
              <Plus className="size-3.5" aria-hidden="true" />
              New session
            </Button>
          </div>
        </header>

        <section className="mt-6 overflow-x-auto rounded-lg border border-border bg-card shadow-card">
          <div
            className={`${LEDGER_GRID} h-9 border-b border-border bg-muted px-5`}
            aria-hidden="true"
          >
            {['WORKSPACE', 'BRANCH', 'CHANGES', 'SESSIONS', 'LAST ACTIVITY'].map((label) => (
              <span
                key={label}
                className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
              >
                {label}
              </span>
            ))}
          </div>

          {index.isLoading ? (
            <LoadingState label="Loading workspaces…" />
          ) : index.isError && !index.data ? (
            <ErrorState
              title="Could not load workspaces"
              message={getErrorMessage(index.error)}
              onRetry={() => void index.refetch()}
            />
          ) : locations.length === 0 ? (
            <EmptyState
              title="No workspaces yet"
              description="Register a directory on a connected machine to start working in it."
              icon={<GitBranch className="size-4" aria-hidden="true" />}
              action={
                <Button
                  variant="primary"
                  onClick={() => setAddWorkspaceOpen(true)}
                  disabled={machines.length === 0}
                >
                  <Plus className="size-3.5" aria-hidden="true" />
                  Add workspace
                </Button>
              }
            />
          ) : (
            groups.map((group) => {
              const machineStatus = machineStatusMeta(group.machine.status);
              return (
                <div key={group.machine.id}>
                  <div className="flex items-center gap-2 border-b border-border bg-muted px-5 py-2.5">
                    <StatusDot tone={machineStatus.tone} size="sm" label={machineStatus.label} />
                    <span className="text-[12px] font-bold text-foreground">
                      {group.machine.name}
                    </span>
                    <span className="font-mono text-[9px] text-faint">
                      {group.machine.os} · {group.machine.arch}
                    </span>
                    <span className="ml-auto font-mono text-[9px] text-faint">
                      {plural(group.locations.length, 'workspace')}
                    </span>
                  </div>

                  {group.locations.map((location) => (
                    <WorkspaceRow
                      key={location.workspace.id}
                      location={location}
                      sessions={sessionsByWorkspace.get(location.workspace.id) ?? []}
                      onOpen={() => openWorkspace(location.workspace.id)}
                    />
                  ))}
                </div>
              );
            })
          )}
        </section>
      </div>

      <AddWorkspaceDialog
        open={addWorkspaceOpen}
        onOpenChange={setAddWorkspaceOpen}
        machines={machines}
      />
      <NewSessionDialog
        open={newSessionOpen}
        onOpenChange={setNewSessionOpen}
        machines={machines}
        onCreated={(session) => {
          void navigate({ to: '/sessions/$sessionId', params: { sessionId: session.id } });
        }}
      />
    </div>
  );
}

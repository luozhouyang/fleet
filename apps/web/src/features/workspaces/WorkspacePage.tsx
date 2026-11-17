import { Link, useNavigate, useParams } from '@tanstack/react-router';
import { GitBranch, Plus } from 'lucide-react';
import { useState } from 'react';
import { NewSessionDialog } from '@/components/dialogs/new-session-dialog';
import { Button, buttonVariants } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { useGitStatus, useWorkspaceLocation } from '@/lib/query/hooks';
import { getErrorMessage } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';
import { WorkspaceChangesPane } from './WorkspaceChangesPane';
import { WorkspaceEditorPane, type EditorTab } from './WorkspaceEditorPane';
import { WorkspaceFileTree } from './WorkspaceFileTree';

const CRUMB_CLASS =
  'font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint transition-colors hover:text-foreground';

function tabName(path: string): string {
  return path.split('/').pop() ?? path;
}

export function WorkspacePage() {
  const params = useParams({ strict: false });
  const workspaceId = typeof params.workspaceId === 'string' ? params.workspaceId : '';
  const navigate = useNavigate();

  const locationQuery = useWorkspaceLocation(workspaceId);
  const location = locationQuery.location;
  const statusQuery = useGitStatus(workspaceId, location?.workspace.is_git_repository ?? false);

  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [tabs, setTabs] = useState<EditorTab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);

  const [stateWorkspaceId, setStateWorkspaceId] = useState(workspaceId);
  if (stateWorkspaceId !== workspaceId) {
    setStateWorkspaceId(workspaceId);
    setTabs([]);
    setActiveTabId(null);
  }

  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? null;

  const openTab = (tab: EditorTab) => {
    setTabs((current) =>
      current.some((candidate) => candidate.id === tab.id) ? current : [...current, tab],
    );
    setActiveTabId(tab.id);
  };

  const openFile = (path: string) => {
    openTab({
      id: `file:${path}`,
      kind: 'file',
      path,
      name: tabName(path),
    });
  };

  const openDiff = (path: string, staged: boolean) => {
    openTab({
      id: `diff:${staged ? 'staged' : 'worktree'}:${path}`,
      kind: 'diff',
      path,
      name: tabName(path),
      staged,
    });
  };

  const closeTab = (id: string) => {
    const index = tabs.findIndex((tab) => tab.id === id);
    if (index === -1) return;
    const next = tabs.filter((tab) => tab.id !== id);
    setTabs(next);
    if (activeTabId === id) {
      const fallback = next[index] ?? next[index - 1] ?? null;
      setActiveTabId(fallback?.id ?? null);
    }
  };

  if (locationQuery.isLoading) {
    return (
      <div className="flex h-full items-center justify-center px-10">
        <LoadingState label="Loading workspace…" />
      </div>
    );
  }

  if (locationQuery.isError && !locationQuery.data) {
    return (
      <div className="flex h-full items-center justify-center px-10">
        <ErrorState
          title="Could not load workspace"
          message={getErrorMessage(locationQuery.error)}
          onRetry={() => void locationQuery.refetch()}
        />
      </div>
    );
  }

  if (!location) {
    return (
      <div className="flex h-full items-center justify-center px-10">
        <EmptyState
          title="Workspace not found"
          description="This workspace may have been removed from its machine."
          action={
            <Link to="/workspaces" className="text-[12px] font-bold text-primary hover:underline">
              Back to workspaces
            </Link>
          }
        />
      </div>
    );
  }

  const { machine, workspace } = location;
  const isGit = workspace.is_git_repository;
  const status = statusQuery.data;
  const statusLoading = statusQuery.isLoading;
  const changeCount = new Set(status?.files?.map((file) => file.path) ?? []).size;
  const branchLabel = status?.branch ?? (status?.detached ? 'detached' : '—');

  return (
    <div className="flex h-full min-h-0 flex-col px-10 pb-6 pt-7">
      <header className="shrink-0">
        <nav aria-label="Breadcrumb" className="flex items-center gap-2">
          <Link to="/workspaces" className={CRUMB_CLASS}>
            Workspaces
          </Link>
          <span className="font-mono text-[9px] text-border-strong" aria-hidden="true">
            /
          </span>
          <Link
            to="/machines/$machineId"
            params={{ machineId: machine.id }}
            className={CRUMB_CLASS}
          >
            {machineDisplayName(machine)}
          </Link>
          <span className="font-mono text-[9px] text-border-strong" aria-hidden="true">
            /
          </span>
          <span className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-muted-foreground">
            {workspace.name}
          </span>
        </nav>

        <div className="mt-1.5 flex flex-wrap items-start justify-between gap-6">
          <div className="min-w-0">
            <div className="flex min-w-0 flex-wrap items-center gap-3">
              <h1 className="truncate font-display text-[24px] font-bold leading-8 tracking-[-0.015em] text-foreground">
                {workspace.name}
              </h1>
              {isGit ? (
                <span className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-inset px-2 py-1 font-mono text-[10px] text-body">
                  <GitBranch className="size-3 text-faint" aria-hidden="true" />
                  {branchLabel}
                </span>
              ) : (
                <span className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-inset px-2 py-1 font-mono text-[10px] text-faint">
                  not git
                </span>
              )}
            </div>

            <p className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-[9px] text-faint">
              <span className="truncate">{workspace.root_path}</span>
              <span aria-hidden="true">·</span>
              <span>
                {isGit
                  ? `${statusLoading ? '…' : changeCount} changed ${
                      changeCount === 1 ? 'file' : 'files'
                    }`
                  : 'not a git repository'}
              </span>
            </p>
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <Link
              to="/machines/$machineId"
              params={{ machineId: machine.id }}
              className={buttonVariants({ variant: 'secondary' })}
            >
              Machine
            </Link>
            <Button variant="primary" onClick={() => setNewSessionOpen(true)}>
              <Plus className="size-3.5" aria-hidden="true" />
              New session
            </Button>
          </div>
        </div>
      </header>

      <div className="mt-5 flex min-h-0 flex-1 gap-4 overflow-x-auto pb-1">
        <WorkspaceFileTree
          key={`tree:${workspace.id}`}
          workspaceId={workspace.id}
          selectedPath={activeTab?.path}
          onOpenFile={openFile}
        />

        <WorkspaceEditorPane
          workspaceId={workspace.id}
          tabs={tabs}
          activeTabId={activeTabId}
          onSelect={setActiveTabId}
          onClose={closeTab}
        />

        <WorkspaceChangesPane
          key={`changes:${workspace.id}`}
          workspaceId={workspace.id}
          isGit={isGit}
          status={status}
          statusLoading={statusLoading}
          statusError={
            statusQuery.isError && !statusQuery.data
              ? getErrorMessage(statusQuery.error)
              : undefined
          }
          activePath={activeTab?.path}
          onOpenDiff={openDiff}
        />
      </div>

      <NewSessionDialog
        key={`new-session:${workspace.id}`}
        open={newSessionOpen}
        onOpenChange={setNewSessionOpen}
        machines={locationQuery.data ?? []}
        defaultMachineId={machine.id}
        defaultWorkspaceId={workspace.id}
        onCreated={(session) => {
          void navigate({ to: '/sessions/$sessionId', params: { sessionId: session.id } });
        }}
      />
    </div>
  );
}

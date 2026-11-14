import type { AgentSession } from '@agent-fleet/protocol-types';
import {
  Badge,
  Button,
  Card,
  CardHeader,
  EmptyState,
  PageHeader,
  StatusDot,
} from '@agent-fleet/ui';
import { useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate, useParams } from '@tanstack/react-router';
import { useMemo, useState } from 'react';
import { NewSessionDialog } from '@/components/dialogs/NewSessionDialog';
import {
  isSessionActive,
  sessionLabel,
  sessionStatusTone,
} from '@/components/status/session-status';
import { useMachines, useSessionList } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { formatRelativeTime, shortId } from '@/lib/utils/format';
import { DiffViewer } from './DiffViewer';
import { DirectoryTree } from './DirectoryTree';
import { ExplorerTabs } from './ExplorerTabs';
import { FileViewer } from './FileViewer';
import { GitPanel } from './GitPanel';
import {
  makeDiffTab,
  makeFileTab,
  type ExplorerTab,
} from './workspace-types';

export function WorkspacePage() {
  const params = useParams({ strict: false });
  const workspaceId = typeof params.workspaceId === 'string' ? params.workspaceId : '';
  const machinesQuery = useMachines();
  const sessionsQuery = useSessionList();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const context = useMemo(() => {
    for (const machine of machinesQuery.data ?? []) {
      const workspace = (machine.workspaces ?? []).find(
        (entry) => entry.id === workspaceId,
      );
      if (workspace) return { machine, workspace };
    }
    return null;
  }, [machinesQuery.data, workspaceId]);

  const workspaceSessions = useMemo(
    () =>
      [...(sessionsQuery.data ?? [])]
        .filter((session) => session.workspace_id === workspaceId)
        .sort((left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at)),
    [sessionsQuery.data, workspaceId],
  );

  const [showHidden, setShowHidden] = useState(false);
  const [tabs, setTabs] = useState<ExplorerTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [newSessionOpen, setNewSessionOpen] = useState(false);

  const activeTab = tabs.find((tab) => tab.id === activeId) ?? null;

  const openFile = (path: string) => {
    const tab = makeFileTab(path);
    setTabs((current) =>
      current.some((candidate) => candidate.id === tab.id) ? current : [...current, tab],
    );
    setActiveId(tab.id);
  };

  const openDiff = (path: string, staged: boolean, includeUntracked: boolean) => {
    const tab = makeDiffTab(path, staged, includeUntracked);
    setTabs((current) =>
      current.some((candidate) => candidate.id === tab.id) ? current : [...current, tab],
    );
    setActiveId(tab.id);
  };

  const closeTab = (id: string) => {
    const index = tabs.findIndex((tab) => tab.id === id);
    const next = tabs.filter((tab) => tab.id !== id);
    setTabs(next);
    if (activeId === id) {
      setActiveId(next[Math.min(index, next.length - 1)]?.id ?? null);
    }
  };

  const workspace = context?.workspace;
  const machine = context?.machine;
  const isGitRepository = workspace?.is_git_repository;
  const selectedPath = activeTab?.path ?? null;
  const availableProviders = (machine?.providers ?? []).filter(
    (provider) => provider.available,
  );
  const canCreateSession = Boolean(machine && workspace && availableProviders.length);

  return (
    <div className="space-y-4">
      <PageHeader
        eyebrow={
          <span className="flex items-center gap-2">
            <Link to="/machines" className="hover:text-teal-200">
              Machines
            </Link>
            <span className="text-slate-700">/</span>
            {machine ? (
              <Link
                to="/machines/$machineId"
                params={{ machineId: machine.id }}
                className="hover:text-teal-200"
              >
                {machine.name}
              </Link>
            ) : (
              <span>Workspace</span>
            )}
          </span>
        }
        title={workspace?.name ?? `Workspace ${shortId(workspaceId, 10)}`}
        description={
          workspace ? (
            <span className="font-mono text-xs text-slate-500">{workspace.root_path}</span>
          ) : machinesQuery.isLoading ? (
            'Resolving workspace metadata…'
          ) : (
            'Workspace metadata is derived from the machine registry.'
          )
        }
        actions={
          <>
            {isGitRepository !== undefined ? (
              <Badge tone={isGitRepository ? 'teal' : 'slate'}>
                {isGitRepository ? 'Git repository' : 'No Git repository'}
              </Badge>
            ) : null}
            <Button
              variant="primary"
              onClick={() => setNewSessionOpen(true)}
              disabled={!canCreateSession}
              title={
                canCreateSession
                  ? `New session in ${workspace?.name ?? 'this workspace'}`
                  : 'Register a workspace and connect a provider on this machine first'
              }
            >
              New session
            </Button>
            {machine ? (
              <Link
                to="/machines/$machineId"
                params={{ machineId: machine.id }}
                className="inline-flex h-9 items-center justify-center rounded-md border border-slate-700 bg-slate-900 px-4 text-sm font-medium text-slate-200 transition-colors hover:border-slate-600 hover:bg-slate-800"
              >
                Machine details
              </Link>
            ) : null}
          </>
        }
      />

      <div className="grid gap-4 lg:grid-cols-[280px_minmax(0,1fr)] xl:grid-cols-[280px_minmax(0,1fr)_360px]">
        <div className="flex min-h-0 flex-col gap-4 lg:h-[calc(100dvh-12rem)] lg:min-h-[520px]">
          <Card className="flex max-h-[240px] min-h-0 shrink-0 flex-col overflow-hidden">
            <CardHeader
              title="Sessions"
              description={`${workspaceSessions.length} in this workspace`}
              action={
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => setNewSessionOpen(true)}
                  disabled={!canCreateSession}
                >
                  New
                </Button>
              }
            />
            <div className="min-h-0 flex-1 overflow-y-auto">
              {workspaceSessions.length === 0 ? (
                <p className="px-4 py-3 text-xs text-slate-500">
                  No sessions yet. Create one to start a coding agent in this workspace.
                </p>
              ) : (
                <ul className="divide-y divide-slate-800/60">
                  {workspaceSessions.map((session) => (
                    <li key={session.id}>
                      <WorkspaceSessionRow session={session} />
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </Card>

          <Card className="flex h-[400px] min-h-0 flex-1 flex-col overflow-hidden">
            <CardHeader
              title="Files"
              description="Directories load only when expanded."
            />
            <DirectoryTree
              workspaceId={workspaceId}
              selectedPath={selectedPath}
              onOpenFile={openFile}
              showHidden={showHidden}
              onShowHiddenChange={setShowHidden}
            />
          </Card>
        </div>

        <Card className="flex h-[560px] min-w-0 flex-col overflow-hidden lg:h-[calc(100dvh-12rem)] lg:min-h-[520px]">
          {tabs.length ? (
            <ExplorerTabs
              tabs={tabs}
              activeId={activeId}
              onSelect={setActiveId}
              onClose={closeTab}
            />
          ) : null}
          <div className="min-h-0 flex-1">
            {activeTab?.kind === 'file' ? (
              <FileViewer
                key={activeTab.id}
                workspaceId={workspaceId}
                path={activeTab.path}
              />
            ) : activeTab?.kind === 'diff' ? (
              <DiffViewer
                key={activeTab.id}
                workspaceId={workspaceId}
                path={activeTab.path}
                staged={activeTab.staged}
                includeUntracked={activeTab.includeUntracked}
              />
            ) : (
              <EmptyState
                title="Nothing open"
                description="Open a file from the explorer, or select a changed file in the Git panel to inspect its diff."
                className="h-full"
                icon={
                  <svg viewBox="0 0 24 24" fill="none" className="size-5" aria-hidden="true">
                    <path
                      d="M5 3.5h9L19 8v12.5H5V3.5Z"
                      stroke="currentColor"
                      strokeWidth="1.5"
                      strokeLinejoin="round"
                    />
                    <path d="M14 3.5V8h5M8 12h8M8 15.5h5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                  </svg>
                }
              />
            )}
          </div>
        </Card>

        <GitPanel
          workspaceId={workspaceId}
          isGitRepository={isGitRepository}
          onOpenDiff={openDiff}
          className="h-[480px] lg:col-span-2 lg:h-[430px] xl:col-span-1 xl:h-[calc(100dvh-12rem)] xl:min-h-[520px]"
        />
      </div>

      {machine && workspace ? (
        <NewSessionDialog
          open={newSessionOpen}
          machine={machine}
          providers={machine.providers ?? []}
          defaultWorkspaceId={workspace.id}
          onClose={() => setNewSessionOpen(false)}
          onCreated={(session) => {
            setNewSessionOpen(false);
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
            void navigate({
              to: '/sessions/$sessionId',
              params: { sessionId: session.id },
            });
          }}
        />
      ) : null}
    </div>
  );
}

function WorkspaceSessionRow({ session }: { session: AgentSession }) {
  return (
    <Link
      to="/sessions/$sessionId"
      params={{ sessionId: session.id }}
      className="flex items-center gap-2.5 px-4 py-2.5 transition-colors hover:bg-slate-900/50"
    >
      <StatusDot
        tone={sessionStatusTone(session.status)}
        pulse={isSessionActive(session.status)}
        className="size-1.5 shrink-0"
      />
      <span className="min-w-0 flex-1 truncate text-xs font-medium text-slate-200">
        {sessionLabel(session)}
      </span>
      <span className="shrink-0 text-[10px] text-slate-500">
        {session.provider} · {formatRelativeTime(session.updated_at)}
      </span>
    </Link>
  );
}

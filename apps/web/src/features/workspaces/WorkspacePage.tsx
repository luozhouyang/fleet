import { Badge, Card, CardHeader, EmptyState, PageHeader } from '@agent-fleet/ui';
import { Link, useParams } from '@tanstack/react-router';
import { useMemo, useState } from 'react';
import { useMachines } from '@/lib/query/hooks';
import { shortId } from '@/lib/utils/format';
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

  const context = useMemo(() => {
    for (const machine of machinesQuery.data ?? []) {
      const workspace = (machine.workspaces ?? []).find(
        (entry) => entry.id === workspaceId,
      );
      if (workspace) return { machine, workspace };
    }
    return null;
  }, [machinesQuery.data, workspaceId]);

  const [showHidden, setShowHidden] = useState(false);
  const [tabs, setTabs] = useState<ExplorerTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);

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
        <Card className="flex h-[400px] flex-col overflow-hidden lg:h-[calc(100dvh-12rem)] lg:min-h-[520px]">
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
    </div>
  );
}

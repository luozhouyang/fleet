import { useEffect, useMemo, useState } from 'react';
import {
  File as FileIcon,
  GitCompare,
  ListTodo,
  PanelRightClose,
  PanelRightOpen,
  Settings2,
  SquareTerminal,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { Tabs, TabsList, TabsPanel, TabsTrigger } from '@/components/ui/tabs';
import { useMachines, useSessionDetail } from '@/lib/query/hooks';
import { effectiveSessionStatus } from '@/lib/session-reducer';
import { useSessionStore } from '@/lib/session-store';
import { getErrorMessage } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';
import { cn } from '@/lib/utils/cn';
import { DiffTab } from './diff-tab';
import { FilePreview } from './file-preview';
import { FileTree } from './file-tree';
import { SessionPlanTab } from './session-plan-tab';
import { SessionTerminalsTab } from './session-terminals-tab';
import { SystemTab } from './system-tab';

const RAIL_ITEMS = [
  { value: 'file', label: 'File', icon: FileIcon },
  { value: 'diff', label: 'Diff', icon: GitCompare },
  { value: 'plan', label: 'Plan', icon: ListTodo },
  { value: 'terminals', label: 'Terminals', icon: SquareTerminal },
  { value: 'system', label: 'System', icon: Settings2 },
] as const;

export function SessionContextPanel({
  sessionId,
  open,
  onToggle,
}: {
  sessionId: string;
  open: boolean;
  onToggle: () => void;
}) {
  const [tab, setTab] = useState('file');
  const [filePath, setFilePath] = useState('');
  const [diffPath, setDiffPath] = useState('');
  const detailQuery = useSessionDetail(sessionId);
  const machinesQuery = useMachines();
  const view = useSessionStore((state) => state.sessions[sessionId]);

  const session = view?.session ?? detailQuery.data?.session ?? null;
  const machine = useMemo(
    () => machinesQuery.data?.find((candidate) => candidate.id === session?.machine_id),
    [machinesQuery.data, session?.machine_id],
  );
  const workspace = useMemo(
    () => machine?.workspaces?.find((candidate) => candidate.id === session?.workspace_id),
    [machine, session?.workspace_id],
  );
  const status = session
    ? ((view ? effectiveSessionStatus(view) : session.status) ?? session.status)
    : undefined;

  useEffect(() => {
    setFilePath('');
    setDiffPath('');
  }, [sessionId]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      if (document.querySelector('[role="dialog"]')) return;
      onToggle();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [open, onToggle]);

  const workspaceId = session?.workspace_id ?? '';

  return (
    <aside
      aria-label="Session context"
      className={cn(
        'flex min-h-0 shrink-0 flex-col overflow-hidden border-l border-border bg-panel transition-[width] duration-200 ease-[cubic-bezier(0.22,1,0.36,1)]',
        open
          ? 'fixed bottom-0 right-0 top-14 z-40 w-[400px] max-w-[88vw] shadow-modal lg:static lg:inset-auto lg:z-auto lg:w-[360px] lg:max-w-none lg:shadow-none xl:w-[400px]'
          : 'hidden w-11 lg:flex',
      )}
    >
      {open ? (
        <div className="flex min-h-0 flex-1 animate-fade flex-col">
          {!session || !status ? (
            <>
              <div className="flex h-11 shrink-0 items-center justify-between gap-2 border-b border-border pl-3 pr-2">
                <span className="font-mono text-[9px] uppercase tracking-[0.12em] text-faint">
                  Context
                </span>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Collapse context panel"
                  title="Collapse context panel"
                  onClick={onToggle}
                >
                  <PanelRightClose className="size-4" />
                </Button>
              </div>
              {detailQuery.isError ? (
                <ErrorState
                  title="Could not load session"
                  message={getErrorMessage(detailQuery.error)}
                  onRetry={() => void detailQuery.refetch()}
                  className="flex-1"
                />
              ) : (
                <LoadingState label="Loading session…" className="flex-1" />
              )}
            </>
          ) : (
            <Tabs
              value={tab}
              onValueChange={(value) => setTab(value == null ? 'file' : String(value))}
              className="flex min-h-0 flex-1 flex-col"
            >
              <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border pl-3 pr-2">
                <TabsList className="border-b-0">
                  <TabsTrigger value="file">File</TabsTrigger>
                  <TabsTrigger value="diff">Diff</TabsTrigger>
                  <TabsTrigger value="plan">Plan</TabsTrigger>
                  <TabsTrigger value="terminals">Terminals</TabsTrigger>
                  <TabsTrigger value="system">System</TabsTrigger>
                </TabsList>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Collapse context panel"
                  title="Collapse context panel"
                  onClick={onToggle}
                >
                  <PanelRightClose className="size-4" />
                </Button>
              </div>

              <TabsPanel value="file" className="flex min-h-0 flex-1 flex-col overflow-hidden">
                {workspaceId ? (
                  <div className="flex min-h-0 flex-1 flex-col">
                    <div className="h-[42%] min-h-[110px] shrink-0 overflow-auto border-b border-border py-1">
                      <FileTree
                        workspaceId={workspaceId}
                        selectedPath={filePath}
                        onSelectPath={setFilePath}
                      />
                    </div>
                    <FilePreview workspaceId={workspaceId} path={filePath} />
                  </div>
                ) : (
                  <EmptyState
                    title="No workspace"
                    description="This session is not attached to a workspace."
                    className="flex-1"
                  />
                )}
              </TabsPanel>

              <TabsPanel value="diff" className="flex min-h-0 flex-1 flex-col overflow-hidden">
                {workspaceId ? (
                  <DiffTab
                    workspaceId={workspaceId}
                    selectedPath={diffPath}
                    onSelectPath={setDiffPath}
                    isGitRepository={workspace?.is_git_repository}
                  />
                ) : (
                  <EmptyState
                    title="No workspace"
                    description="This session is not attached to a workspace."
                    className="flex-1"
                  />
                )}
              </TabsPanel>

              <TabsPanel value="plan" className="flex min-h-0 flex-1 flex-col overflow-hidden">
                <SessionPlanTab sessionId={sessionId} />
              </TabsPanel>

              <TabsPanel value="terminals" className="flex min-h-0 flex-1 flex-col overflow-hidden">
                <SessionTerminalsTab sessionId={sessionId} />
              </TabsPanel>

              <TabsPanel value="system" className="flex min-h-0 flex-1 flex-col overflow-hidden">
                <SystemTab
                  session={session}
                  status={status}
                  view={view}
                  detail={detailQuery.data}
                  machineName={machine ? machineDisplayName(machine) : undefined}
                  workspaceName={workspace?.name}
                />
              </TabsPanel>
            </Tabs>
          )}
        </div>
      ) : (
        <div className="flex animate-fade flex-col items-center gap-1 py-2">
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Expand context panel"
            title="Expand context panel"
            onClick={onToggle}
          >
            <PanelRightOpen className="size-4" />
          </Button>
          {RAIL_ITEMS.map((item) => {
            const Icon = item.icon;
            const active = tab === item.value;
            return (
              <button
                key={item.value}
                type="button"
                aria-label={`Open ${item.label} panel`}
                title={item.label}
                onClick={() => {
                  setTab(item.value);
                  onToggle();
                }}
                className={
                  active
                    ? 'flex size-8 items-center justify-center rounded-md bg-selected text-primary transition-colors'
                    : 'flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground'
                }
              >
                <Icon className="size-4" aria-hidden="true" />
              </button>
            );
          })}
        </div>
      )}
    </aside>
  );
}

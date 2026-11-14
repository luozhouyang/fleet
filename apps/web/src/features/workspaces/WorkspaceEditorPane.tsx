import { File, FileDiff, X } from 'lucide-react';
import { CodeEditor } from '@/components/editor/code-editor';
import { DiffEditor } from '@/components/editor/diff-editor';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { useFileContent, useGitDiff } from '@/lib/query/hooks';
import { cn } from '@/lib/utils/cn';
import { formatBytes, getErrorMessage } from '@/lib/utils/format';
import { languageForPath } from '@/lib/utils/language';
import { buildDiffSides, gitStatusClass, gitStatusLetter } from './workspace-utils';

export interface EditorTab {
  id: string;
  kind: 'file' | 'diff';
  path: string;
  name: string;
  staged?: boolean;
}

function FileTabView({ workspaceId, path }: { workspaceId: string; path: string }) {
  const fileQuery = useFileContent(workspaceId, path);

  if (fileQuery.isLoading) return <LoadingState label="Loading file…" />;
  if (fileQuery.isError) {
    return (
      <ErrorState
        title="Could not open file"
        message={getErrorMessage(fileQuery.error)}
        onRetry={() => void fileQuery.refetch()}
      />
    );
  }

  const file = fileQuery.data;
  if (!file) {
    return <EmptyState title="Empty file" description="This file has no content to show." />;
  }
  if (file.is_binary) {
    return (
      <EmptyState
        title="Binary file"
        description="This file cannot be displayed in the text editor."
        icon={<File className="size-4" aria-hidden="true" />}
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {file.truncated ? (
        <div className="shrink-0 border-b border-border bg-warning-soft px-3 py-1.5 text-[11px] text-warning">
          On-disk size {formatBytes(file.size)}; the preview was truncated.
        </div>
      ) : null}
      <div className="min-h-0 flex-1">
        <CodeEditor value={file.content} language={languageForPath(path, file.language)} readOnly />
      </div>
    </div>
  );
}

function DiffTabView({
  workspaceId,
  path,
  staged,
}: {
  workspaceId: string;
  path: string;
  staged: boolean;
}) {
  const diffQuery = useGitDiff(
    workspaceId,
    staged ? { path, staged: true } : { path, include_untracked: true },
  );

  if (diffQuery.isLoading) return <LoadingState label="Loading diff…" />;
  if (diffQuery.isError) {
    return (
      <ErrorState
        title="Could not load diff"
        message={getErrorMessage(diffQuery.error)}
        onRetry={() => void diffQuery.refetch()}
      />
    );
  }

  const file =
    diffQuery.data?.files.find((candidate) => candidate.path === path) ?? diffQuery.data?.files[0];

  if (!file) {
    return (
      <EmptyState
        title="No diff"
        description="This file has no changes against the index."
        icon={<FileDiff className="size-4" aria-hidden="true" />}
      />
    );
  }
  if (file.is_binary) {
    return (
      <EmptyState
        title="Binary diff"
        description="Binary files cannot be shown as text."
        icon={<FileDiff className="size-4" aria-hidden="true" />}
      />
    );
  }

  const { original, modified } = buildDiffSides(file);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-3">
        <span className={cn('font-mono text-[10px] font-bold', gitStatusClass(file.status))}>
          {gitStatusLetter(file.status)}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-body">{file.path}</span>
        <span className="font-mono text-[9px] text-success">+{file.additions}</span>
        <span className="font-mono text-[9px] text-destructive">-{file.deletions}</span>
      </div>
      <div className="min-h-0 flex-1">
        <DiffEditor original={original} modified={modified} language={languageForPath(file.path)} />
      </div>
    </div>
  );
}

export function WorkspaceEditorPane({
  workspaceId,
  tabs,
  activeTabId,
  onSelect,
  onClose,
}: {
  workspaceId: string;
  tabs: EditorTab[];
  activeTabId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
}) {
  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? null;

  return (
    <section className="flex min-h-0 min-w-[360px] flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card shadow-card">
      <div className="flex h-10 shrink-0 items-stretch overflow-x-auto border-b border-border">
        {tabs.length === 0 ? (
          <span className="flex items-center px-3 font-mono text-[9px] uppercase tracking-[0.14em] text-faint">
            NO OPEN FILES
          </span>
        ) : (
          tabs.map((tab) => {
            const active = tab.id === activeTab?.id;
            return (
              <div
                key={tab.id}
                className={cn(
                  'flex shrink-0 items-center border-b-2 px-1 transition-colors',
                  active ? 'border-primary' : 'border-transparent',
                )}
              >
                <button
                  type="button"
                  onClick={() => onSelect(tab.id)}
                  title={tab.path}
                  aria-current={active ? 'page' : undefined}
                  className={cn(
                    'flex h-9 max-w-[220px] items-center gap-1.5 pl-2 text-[11.5px] outline-none focus-visible:ring-2 focus-visible:ring-ring/35',
                    active
                      ? 'font-medium text-foreground'
                      : 'text-muted-foreground hover:text-foreground',
                  )}
                >
                  {tab.kind === 'diff' ? (
                    <FileDiff className="size-3 shrink-0 text-faint" aria-hidden="true" />
                  ) : (
                    <File className="size-3 shrink-0 text-faint" aria-hidden="true" />
                  )}
                  <span className="truncate">{tab.name}</span>
                </button>
                <button
                  type="button"
                  onClick={() => onClose(tab.id)}
                  aria-label={`Close ${tab.name}`}
                  title={`Close ${tab.name}`}
                  className="ml-0.5 flex size-6 shrink-0 items-center justify-center rounded text-faint transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/35"
                >
                  <X className="size-3" aria-hidden="true" />
                </button>
              </div>
            );
          })
        )}
      </div>

      <div className="min-h-0 flex-1">
        {activeTab ? (
          activeTab.kind === 'file' ? (
            <FileTabView key={activeTab.id} workspaceId={workspaceId} path={activeTab.path} />
          ) : (
            <DiffTabView
              key={activeTab.id}
              workspaceId={workspaceId}
              path={activeTab.path}
              staged={Boolean(activeTab.staged)}
            />
          )
        ) : (
          <EmptyState
            title="Open a file or a changed file"
            description="Pick a file from the tree or select a change to view its diff."
            icon={<FileDiff className="size-4" aria-hidden="true" />}
          />
        )}
      </div>
    </section>
  );
}

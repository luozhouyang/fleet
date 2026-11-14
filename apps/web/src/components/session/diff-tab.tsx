import { DiffLineType, type DiffLine, type FileDiff } from '@agent-fleet/protocol-types';
import { Link } from '@tanstack/react-router';
import { ArrowRight, File as FileIcon } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { useGitDiff, useGitDiffSummary } from '@/lib/query/hooks';
import { cn } from '@/lib/utils/cn';
import { getErrorMessage } from '@/lib/utils/format';

export function DiffTab({
  workspaceId,
  selectedPath,
  onSelectPath,
  isGitRepository,
}: {
  workspaceId: string;
  selectedPath: string;
  onSelectPath: (path: string) => void;
  isGitRepository?: boolean;
}) {
  const [staged, setStaged] = useState(false);
  const summaryOptions = useMemo(
    () => (staged ? { staged: true } : { include_untracked: true }),
    [staged],
  );
  const summaryQuery = useGitDiffSummary(workspaceId, summaryOptions, Boolean(workspaceId));
  const files = summaryQuery.data?.files;

  useEffect(() => {
    if (!files?.length) return;
    if (!files.some((file) => file.path === selectedPath)) {
      onSelectPath(files[0].path);
    }
  }, [files, selectedPath, onSelectPath]);

  useEffect(() => {
    if (summaryQuery.data && summaryQuery.data.files.length === 0 && selectedPath) {
      onSelectPath('');
    }
  }, [summaryQuery.data, selectedPath, onSelectPath]);

  const diffOptions = useMemo(
    () =>
      staged
        ? { path: selectedPath || undefined, staged: true }
        : { path: selectedPath || undefined, include_untracked: true },
    [selectedPath, staged],
  );
  const diffQuery = useGitDiff(
    workspaceId,
    diffOptions,
    Boolean(workspaceId) && Boolean(selectedPath),
  );
  const file = diffQuery.data
    ? (diffQuery.data.files.find((candidate) => candidate.path === selectedPath) ??
      diffQuery.data.files[0])
    : undefined;
  const summary = summaryQuery.data;

  if (isGitRepository === false) {
    return (
      <EmptyState
        title="No Git repository"
        description="This workspace is not tracked by Git, so there are no changes to diff."
        className="flex-1"
      />
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border px-3 py-1.5">
        <span className="flex shrink-0 items-center gap-0.5 font-mono text-[9px]">
          <button
            type="button"
            onClick={() => setStaged(false)}
            aria-pressed={!staged}
            className={cn(
              'rounded-sm px-1.5 py-0.5 transition-colors',
              staged ? 'text-muted-foreground hover:text-foreground' : 'bg-selected text-primary',
            )}
          >
            Worktree
          </button>
          <button
            type="button"
            onClick={() => setStaged(true)}
            aria-pressed={staged}
            className={cn(
              'rounded-sm px-1.5 py-0.5 transition-colors',
              staged ? 'bg-selected text-primary' : 'text-muted-foreground hover:text-foreground',
            )}
          >
            Staged
          </button>
        </span>
        <span className="shrink-0 font-mono text-[9.5px] text-faint">
          {summary?.branch ?? 'worktree'} · {files?.length ?? 0} files
        </span>
      </div>

      <div className="max-h-[38%] shrink-0 overflow-auto border-b border-border py-1">
        {summaryQuery.isLoading ? (
          <LoadingState label="Loading changes…" className="py-6" />
        ) : summaryQuery.isError ? (
          <ErrorState
            title="Could not load changes"
            message={getErrorMessage(summaryQuery.error)}
            onRetry={() => void summaryQuery.refetch()}
            className="py-6"
          />
        ) : !files?.length ? (
          <EmptyState
            title="No changes"
            description={
              staged
                ? 'Nothing is staged in the index.'
                : 'The workspace has no working-tree changes.'
            }
            className="py-6"
          />
        ) : (
          files.map((file) => {
            const selected = file.path === selectedPath;
            return (
              <button
                key={file.path}
                type="button"
                onClick={() => onSelectPath(file.path)}
                title={file.path}
                data-testid="session-diff-file"
                data-path={file.path}
                className={cn(
                  'flex w-full items-center gap-2 px-3 py-1 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40',
                  selected ? 'bg-selected' : 'hover:bg-muted',
                )}
              >
                <FileIcon className="size-3 shrink-0 text-faint" aria-hidden="true" />
                <span
                  className={cn(
                    'min-w-0 flex-1 truncate font-mono text-[10.5px]',
                    selected ? 'text-foreground' : 'text-body',
                  )}
                >
                  {file.path}
                </span>
                <span className="shrink-0 font-mono text-[9.5px] text-success">
                  +{file.additions}
                </span>
                <span className="shrink-0 font-mono text-[9.5px] text-destructive">
                  −{file.deletions}
                </span>
              </button>
            );
          })
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {diffQuery.isLoading ? (
          <LoadingState label="Loading diff…" className="py-10" />
        ) : diffQuery.isError ? (
          <ErrorState
            title="Could not load diff"
            message={getErrorMessage(diffQuery.error)}
            onRetry={() => void diffQuery.refetch()}
            className="py-10"
          />
        ) : !file ? (
          <EmptyState
            title="Select a file"
            description="Choose a changed file to view its unified diff."
            className="py-10"
          />
        ) : file.is_binary ? (
          <EmptyState
            title="Binary diff"
            description="A textual diff cannot be rendered for this file."
            className="py-10"
          />
        ) : file.hunks.length === 0 ? (
          <EmptyState
            title="No textual changes"
            description="This file has no renderable hunks."
            className="py-10"
          />
        ) : (
          <DiffBody file={file} />
        )}
      </div>

      <div className="flex shrink-0 items-center justify-between gap-2 border-t border-border px-3 py-2">
        <span className="truncate font-mono text-[9px] text-faint">
          {file ? `${file.additions} added · ${file.deletions} removed` : 'unified diff'}
        </span>
        <Link
          to="/workspaces/$workspaceId"
          params={{ workspaceId }}
          className="inline-flex shrink-0 items-center gap-1 text-[10.5px] font-bold text-primary hover:underline"
        >
          Open workspace
          <ArrowRight className="size-3" aria-hidden="true" />
        </Link>
      </div>
    </div>
  );
}

function DiffBody({ file }: { file: FileDiff }) {
  return (
    <div className="min-w-max">
      {file.hunks.map((hunk, hunkIndex) => (
        <div key={`${hunk.old_start}-${hunk.new_start}-${hunkIndex}`}>
          <div className="flex gap-2 border-y border-border bg-muted px-2 py-0.5 font-mono text-[9.5px] text-faint">
            <span className="shrink-0">
              @@ -{hunk.old_start},{hunk.old_lines} +{hunk.new_start},{hunk.new_lines} @@
            </span>
            {hunk.header ? <span className="truncate">{hunk.header}</span> : null}
          </div>
          {hunk.lines.map((line, lineIndex) => (
            <DiffRow key={`${hunkIndex}-${lineIndex}`} line={line} />
          ))}
        </div>
      ))}
    </div>
  );
}

function DiffRow({ line }: { line: DiffLine }) {
  const added = line.line_type === DiffLineType.Added;
  const removed = line.line_type === DiffLineType.Deleted;

  return (
    <div
      className={cn(
        'grid grid-cols-[3rem_3rem_1rem_1fr] font-mono text-[10.5px] leading-[17px]',
        added && 'bg-success-soft',
        removed && 'bg-destructive-soft',
      )}
    >
      <span className="border-r border-border/60 pr-1.5 text-right text-[9px] text-faint">
        {line.old_line_no ?? ''}
      </span>
      <span className="border-r border-border/60 pr-1.5 text-right text-[9px] text-faint">
        {line.new_line_no ?? ''}
      </span>
      <span className={cn('text-center', added && 'text-success', removed && 'text-destructive')}>
        {added ? '+' : removed ? '−' : ' '}
      </span>
      <span className="whitespace-pre pr-3 text-body">{line.content || ' '}</span>
    </div>
  );
}

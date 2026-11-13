import { Badge, Button, EmptyState, Spinner } from '@agent-fleet/ui';
import { useMemo } from 'react';
import { FileDiffEditor } from '@/components/editor/FileDiffEditor';
import { diffSides } from '@/components/editor/diff-text';
import { GitStatusLetter } from '@/components/status/StatusBadge';
import { useGitDiff } from '@/lib/query/hooks';
import { getErrorMessage } from '@/lib/utils/format';
import { languageForPath } from '@/lib/utils/language';

export function DiffViewer({
  workspaceId,
  path,
  staged,
  includeUntracked,
}: {
  workspaceId: string;
  path: string;
  staged: boolean;
  includeUntracked: boolean;
}) {
  const options = useMemo(
    () => ({
      path,
      staged,
      include_untracked: includeUntracked,
      context_lines: 3,
    }),
    [path, staged, includeUntracked],
  );
  const query = useGitDiff(workspaceId, options, true);
  const file = query.data
    ? (query.data.files.find((entry) => entry.path === path) ?? query.data.files[0])
    : undefined;
  const sides = useMemo(() => (file ? diffSides(file) : null), [file]);

  if (query.isLoading) {
    return (
      <div className="flex h-full items-center justify-center gap-3 text-xs text-slate-500">
        <Spinner size="sm" label="Loading diff" />
        Loading diff…
      </div>
    );
  }

  if (query.isError) {
    return (
      <EmptyState
        title="Could not load diff"
        description={getErrorMessage(query.error)}
        className="h-full"
        action={
          <Button variant="secondary" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        }
      />
    );
  }

  if (!file) {
    return (
      <EmptyState
        title="No textual diff"
        description="The workspace service did not return a diff for this file."
        className="h-full"
      />
    );
  }

  if (query.data?.is_binary || file.is_binary) {
    return (
      <EmptyState
        title="Binary diff"
        description="Monaco cannot render a textual diff for binary files."
        className="h-full"
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-slate-800/70 bg-slate-950/25 px-3 py-1.5 text-[10px] text-slate-500">
        <GitStatusLetter status={file.status} />
        <span className="max-w-[45%] truncate font-mono text-slate-400" title={path}>
          {path}
        </span>
        {staged ? <Badge tone="teal">staged</Badge> : null}
        {includeUntracked ? <Badge tone="green">untracked</Badge> : null}
        <span className="font-mono">
          <span className="text-emerald-400">+{file.additions}</span>{' '}
          <span className="text-red-400">−{file.deletions}</span>
        </span>
        <span className="hidden sm:inline">{file.hunks.length} hunks</span>
        <div className="ml-auto flex items-center gap-2">
          {query.isFetching ? <Spinner size="sm" label="Refreshing diff" /> : null}
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-[10px]"
            onClick={() => void query.refetch()}
          >
            Refresh
          </Button>
        </div>
      </div>
      {sides ? (
        <div className="min-h-0 flex-1">
          <FileDiffEditor
            original={sides.original}
            modified={sides.modified}
            language={languageForPath(path)}
          />
        </div>
      ) : (
        <EmptyState
          title="No textual diff"
          description="This file has no renderable hunks."
          className="h-full"
        />
      )}
    </div>
  );
}

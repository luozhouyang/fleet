import {
  GitFileStatus,
  type DiffSummary,
  type GitFileChange,
  type GitStatus,
} from '@agent-fleet/protocol-types';
import { Badge, Card, EmptyState, Spinner, cn } from '@agent-fleet/ui';
import { useQueryClient } from '@tanstack/react-query';
import { GitStatusLetter } from '@/components/status/StatusBadge';
import {
  useGitDiffSummary,
  useGitStatus,
} from '@/lib/query/hooks';
import { getErrorMessage } from '@/lib/utils/format';

export function GitPanel({
  workspaceId,
  isGitRepository,
  onOpenDiff,
  className,
}: {
  workspaceId: string;
  isGitRepository?: boolean;
  onOpenDiff: (path: string, staged: boolean, includeUntracked: boolean) => void;
  className?: string;
}) {
  const queryClient = useQueryClient();
  const enabled = isGitRepository !== false;
  const statusQuery = useGitStatus(workspaceId, enabled);
  const summaryQuery = useGitDiffSummary(
    workspaceId,
    {},
    enabled && Boolean(statusQuery.data),
  );

  const refresh = () => {
    void queryClient.invalidateQueries({
      queryKey: ['workspaces', workspaceId, 'git'],
    });
    void queryClient.invalidateQueries({
      queryKey: ['workspaces', workspaceId, 'tree'],
    });
  };

  if (!enabled) {
    return (
      <Card className={cn('flex flex-col overflow-hidden', className)}>
        <GitPanelHeader title="Git changes" onRefresh={refresh} />
        <EmptyState
          title="Not a Git repository"
          description="Register a Git repository to inspect branches, changes and diffs here."
          className="flex-1"
        />
      </Card>
    );
  }

  if (statusQuery.isLoading) {
    return (
      <Card className={cn('flex flex-col overflow-hidden', className)}>
        <GitPanelHeader title="Git changes" onRefresh={refresh} />
        <div className="flex flex-1 items-center justify-center gap-2 text-xs text-slate-500">
          <Spinner size="sm" label="Loading git status" />
          Reading status…
        </div>
      </Card>
    );
  }

  if (statusQuery.isError) {
    return (
      <Card className={cn('flex flex-col overflow-hidden', className)}>
        <GitPanelHeader title="Git changes" onRefresh={refresh} />
        <EmptyState
          title="Git status unavailable"
          description={getErrorMessage(statusQuery.error)}
          className="flex-1"
          action={
            <button
              type="button"
              onClick={refresh}
              className="text-xs font-medium text-teal-300 hover:text-teal-200"
            >
              Try again
            </button>
          }
        />
      </Card>
    );
  }

  const status = statusQuery.data;
  if (!status) return null;

  const files = status.files ?? [];
  const staged = files.filter((file) => file.staged);
  const untracked = files.filter(
    (file) => !file.staged && file.status === GitFileStatus.Untracked,
  );
  const unstaged = files.filter(
    (file) => !file.staged && file.status !== GitFileStatus.Untracked,
  );
  const summaries = new Map(
    (summaryQuery.data?.files ?? []).map((summary) => [summary.path, summary]),
  );
  const additions = summaryQuery.data?.files.reduce(
    (total, summary) => total + summary.additions,
    0,
  );
  const deletions = summaryQuery.data?.files.reduce(
    (total, summary) => total + summary.deletions,
    0,
  );

  return (
    <Card className={cn('flex flex-col overflow-hidden', className)}>
      <GitPanelHeader
        title="Git changes"
        status={status}
        additions={additions}
        deletions={deletions}
        onRefresh={refresh}
      />
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {files.length === 0 ? (
          <EmptyState
            title="Working tree clean"
            description="There are no staged, unstaged or untracked changes."
            className="h-full"
          />
        ) : (
          <div className="space-y-3">
            <ChangeSection
              title="Staged"
              files={staged}
              summaries={summaries}
              staged
              onOpenDiff={onOpenDiff}
            />
            <ChangeSection
              title="Unstaged"
              files={unstaged}
              summaries={summaries}
              staged={false}
              onOpenDiff={onOpenDiff}
            />
            <ChangeSection
              title="Untracked"
              files={untracked}
              summaries={summaries}
              staged={false}
              untracked
              onOpenDiff={onOpenDiff}
            />
          </div>
        )}
      </div>
    </Card>
  );
}

function GitPanelHeader({
  title,
  status,
  additions,
  deletions,
  onRefresh,
}: {
  title: string;
  status?: GitStatus;
  additions?: number;
  deletions?: number;
  onRefresh: () => void;
}) {
  return (
    <div className="shrink-0 border-b border-slate-800/80 p-3">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="text-sm font-semibold text-slate-100">{title}</span>
          {status ? (
            <Badge tone="neutral" className="max-w-40 truncate font-mono">
              {status.branch ?? (status.detached ? 'detached HEAD' : 'no branch')}
            </Badge>
          ) : null}
        </div>
        <button
          type="button"
          onClick={onRefresh}
          aria-label="Refresh Git status"
          title="Refresh Git status"
          className="flex size-7 shrink-0 items-center justify-center rounded-md text-slate-500 hover:bg-slate-800 hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60"
        >
          <svg viewBox="0 0 20 20" fill="none" className="size-4" aria-hidden="true">
            <path
              d="M15.5 7A6 6 0 1 0 16 12M15.5 3.5V7H12"
              stroke="currentColor"
              strokeWidth="1.4"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>
      </div>

      {status ? (
        <div className="mt-2 flex flex-wrap items-center gap-2 text-[10px] text-slate-500">
          {status.ahead > 0 ? <Badge tone="teal">↑ {status.ahead} ahead</Badge> : null}
          {status.behind > 0 ? <Badge tone="amber">↓ {status.behind} behind</Badge> : null}
          {status.ahead === 0 && status.behind === 0 ? (
            <span className="font-mono">up to date</span>
          ) : null}
          {additions !== undefined && deletions !== undefined ? (
            <span className="font-mono">
              <span className="text-emerald-400">+{additions}</span>{' '}
              <span className="text-red-400">−{deletions}</span>
            </span>
          ) : null}
          <span className="ml-auto font-mono">{status.is_clean ? 'clean' : 'changed'}</span>
        </div>
      ) : null}
    </div>
  );
}

function ChangeSection({
  title,
  files,
  summaries,
  staged,
  untracked = false,
  onOpenDiff,
}: {
  title: string;
  files: GitFileChange[];
  summaries: Map<string, DiffSummary>;
  staged: boolean;
  untracked?: boolean;
  onOpenDiff: (path: string, staged: boolean, includeUntracked: boolean) => void;
}) {
  return (
    <section>
      <div className="flex items-center justify-between px-2 py-1">
        <span className="text-[10px] font-semibold uppercase tracking-[0.14em] text-slate-600">
          {title}
        </span>
        <span className="font-mono text-[10px] text-slate-600">{files.length}</span>
      </div>
      {files.length === 0 ? (
        <p className="px-2 py-1 text-[11px] text-slate-700">None</p>
      ) : (
        <div className="space-y-0.5">
          {files.map((file) => {
            const summary = summaries.get(file.path);
            return (
              <button
                key={`${file.path}:${staged ? 'staged' : untracked ? 'untracked' : 'worktree'}`}
                type="button"
                data-testid="git-file"
                data-path={file.path}
                onClick={() => onOpenDiff(file.path, staged, untracked)}
                className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-slate-800/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/50"
              >
                <GitStatusLetter status={file.status} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-xs text-slate-300" title={file.path}>
                    {file.path}
                  </span>
                  {file.old_path ? (
                    <span className="block truncate text-[10px] text-slate-600">
                      from {file.old_path}
                    </span>
                  ) : null}
                </span>
                {summary ? (
                  <span className="shrink-0 font-mono text-[9px]">
                    <span className="text-emerald-500/80">+{summary.additions}</span>{' '}
                    <span className="text-red-500/80">−{summary.deletions}</span>
                  </span>
                ) : null}
              </button>
            );
          })}
        </div>
      )}
    </section>
  );
}

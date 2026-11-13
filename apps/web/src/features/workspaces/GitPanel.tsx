import {
  GitFileStatus,
  type DiffSummary,
  type GitFileChange,
  type GitStatus,
} from '@agent-fleet/protocol-types';
import {
  Badge,
  Button,
  Card,
  Dialog,
  EmptyState,
  Field,
  Input,
  Select,
  Spinner,
  Textarea,
  cn,
} from '@agent-fleet/ui';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { GitStatusLetter } from '@/components/status/StatusBadge';
import { apiClient } from '@/lib/api/client';
import { useGitBranches, useGitDiffSummary, useGitStatus } from '@/lib/query/hooks';
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
  const summaryQuery = useGitDiffSummary(workspaceId, {}, enabled && Boolean(statusQuery.data));
  const [commitOpen, setCommitOpen] = useState(false);
  const [commitMessage, setCommitMessage] = useState('');
  const [checkoutOpen, setCheckoutOpen] = useState(false);
  const [checkoutBranch, setCheckoutBranch] = useState('');
  const [newBranch, setNewBranch] = useState('');
  const branchesQuery = useGitBranches(workspaceId, enabled && checkoutOpen);

  const refresh = () => {
    void queryClient.invalidateQueries({
      queryKey: ['workspaces', workspaceId, 'git'],
    });
    void queryClient.invalidateQueries({
      queryKey: ['workspaces', workspaceId, 'tree'],
    });
  };

  const stageMutation = useMutation({
    mutationFn: (target: { path: string; staged: boolean }) =>
      target.staged
        ? apiClient.gitUnstage(workspaceId, { paths: [target.path] })
        : apiClient.gitStage(workspaceId, { paths: [target.path] }),
    onSuccess: refresh,
  });

  const commitMutation = useMutation({
    mutationFn: () => apiClient.gitCommit(workspaceId, { message: commitMessage.trim() }),
    onSuccess: () => {
      setCommitOpen(false);
      setCommitMessage('');
      refresh();
    },
  });

  const checkoutMutation = useMutation({
    mutationFn: () => {
      const created = newBranch.trim();
      return apiClient.gitCheckout(
        workspaceId,
        created ? { branch: created, create: true } : { branch: checkoutBranch },
      );
    },
    onSuccess: () => {
      setCheckoutOpen(false);
      setNewBranch('');
      setCheckoutBranch('');
      refresh();
    },
  });

  const openCommit = () => {
    commitMutation.reset();
    setCommitOpen(true);
  };

  const openCheckout = () => {
    checkoutMutation.reset();
    setNewBranch('');
    setCheckoutBranch('');
    setCheckoutOpen(true);
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
  const untracked = files.filter((file) => !file.staged && file.status === GitFileStatus.Untracked);
  const unstaged = files.filter((file) => !file.staged && file.status !== GitFileStatus.Untracked);
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
  const branches = (branchesQuery.data ?? []).filter(
    (branch) => !branch.is_remote && !branch.is_current,
  );
  const checkoutTarget = newBranch.trim() || checkoutBranch;
  const stagePendingPath = stageMutation.isPending ? stageMutation.variables?.path : undefined;

  return (
    <Card className={cn('flex flex-col overflow-hidden', className)}>
      <GitPanelHeader
        title="Git changes"
        status={status}
        additions={additions}
        deletions={deletions}
        onRefresh={refresh}
        onCheckout={openCheckout}
      />

      {staged.length > 0 ? (
        <div className="shrink-0 border-b border-slate-800/80 bg-slate-950/40 px-3 py-2">
          <Button
            variant="primary"
            size="sm"
            data-testid="git-commit-open"
            onClick={openCommit}
            className="w-full"
          >
            Commit {staged.length} staged {staged.length === 1 ? 'file' : 'files'}
          </Button>
        </div>
      ) : null}

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
              pendingPath={stagePendingPath}
              onOpenDiff={onOpenDiff}
              onToggleStage={(path) => stageMutation.mutate({ path, staged: true })}
            />
            <ChangeSection
              title="Unstaged"
              files={unstaged}
              summaries={summaries}
              staged={false}
              pendingPath={stagePendingPath}
              onOpenDiff={onOpenDiff}
              onToggleStage={(path) => stageMutation.mutate({ path, staged: false })}
            />
            <ChangeSection
              title="Untracked"
              files={untracked}
              summaries={summaries}
              staged={false}
              untracked
              pendingPath={stagePendingPath}
              onOpenDiff={onOpenDiff}
              onToggleStage={(path) => stageMutation.mutate({ path, staged: false })}
            />
          </div>
        )}
      </div>

      {stageMutation.isError ? (
        <p className="shrink-0 border-t border-red-500/20 bg-red-500/5 px-3 py-2 text-[11px] leading-5 text-red-300">
          {getErrorMessage(stageMutation.error)}
        </p>
      ) : null}

      <Dialog
        open={commitOpen}
        onClose={() => setCommitOpen(false)}
        title="Commit staged changes"
        description={`${staged.length} staged ${staged.length === 1 ? 'file' : 'files'} on ${status.branch ?? 'detached HEAD'}.`}
        footer={
          <>
            <Button
              variant="ghost"
              onClick={() => setCommitOpen(false)}
              disabled={commitMutation.isPending}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              data-testid="git-commit-submit"
              onClick={() => commitMutation.mutate()}
              loading={commitMutation.isPending}
              disabled={!commitMessage.trim() || commitMutation.isPending}
            >
              Commit
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <Field
            label="Commit message"
            htmlFor="git-commit-message"
            required
            hint="Only the staged files are committed."
          >
            <Textarea
              id="git-commit-message"
              data-testid="git-commit-message"
              value={commitMessage}
              onChange={(event) => setCommitMessage(event.target.value)}
              rows={4}
              autoFocus
              placeholder="Describe the change"
              className="font-mono text-xs"
              disabled={commitMutation.isPending}
            />
          </Field>
          {commitMutation.error ? (
            <p className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs leading-5 text-red-300">
              {getErrorMessage(commitMutation.error)}
            </p>
          ) : null}
        </div>
      </Dialog>

      <Dialog
        open={checkoutOpen}
        onClose={() => setCheckoutOpen(false)}
        title="Switch branch"
        description="Check out an existing branch or create a new one. Local changes that would be overwritten are rejected."
        footer={
          <>
            <Button
              variant="ghost"
              onClick={() => setCheckoutOpen(false)}
              disabled={checkoutMutation.isPending}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              data-testid="git-checkout-submit"
              onClick={() => checkoutMutation.mutate()}
              loading={checkoutMutation.isPending}
              disabled={!checkoutTarget || checkoutMutation.isPending}
            >
              Checkout
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <Field
            label="Existing branch"
            htmlFor="git-checkout-branch"
            hint="Current branch is excluded."
          >
            <Select
              id="git-checkout-branch"
              data-testid="git-checkout-branch"
              value={checkoutBranch}
              onChange={(event) => setCheckoutBranch(event.target.value)}
              disabled={
                branchesQuery.isLoading || branches.length === 0 || checkoutMutation.isPending
              }
            >
              <option value="">
                {branchesQuery.isLoading
                  ? 'Loading branches…'
                  : branches.length === 0
                    ? 'No other local branches'
                    : 'Select a branch'}
              </option>
              {branches.map((branch) => (
                <option key={branch.name} value={branch.name}>
                  {branch.name}
                </option>
              ))}
            </Select>
          </Field>
          <Field
            label="New branch"
            htmlFor="git-checkout-new"
            hint="Optional. Takes precedence over the selection above."
          >
            <Input
              id="git-checkout-new"
              data-testid="git-checkout-new"
              value={newBranch}
              onChange={(event) => setNewBranch(event.target.value)}
              placeholder="feature/my-branch"
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
              disabled={checkoutMutation.isPending}
            />
          </Field>
          {branchesQuery.error ? (
            <p className="text-xs text-red-300">{getErrorMessage(branchesQuery.error)}</p>
          ) : null}
          {checkoutMutation.error ? (
            <p className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs leading-5 text-red-300">
              {getErrorMessage(checkoutMutation.error)}
            </p>
          ) : null}
        </div>
      </Dialog>
    </Card>
  );
}

function GitPanelHeader({
  title,
  status,
  additions,
  deletions,
  onRefresh,
  onCheckout,
}: {
  title: string;
  status?: GitStatus;
  additions?: number;
  deletions?: number;
  onRefresh: () => void;
  onCheckout?: () => void;
}) {
  return (
    <div className="shrink-0 border-b border-slate-800/80 p-3">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="text-sm font-semibold text-slate-100">{title}</span>
          {status ? (
            <Badge tone="neutral" data-testid="git-branch" className="max-w-40 truncate font-mono">
              {status.branch ?? (status.detached ? 'detached HEAD' : 'no branch')}
            </Badge>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {onCheckout ? (
            <button
              type="button"
              onClick={onCheckout}
              data-testid="git-checkout-open"
              aria-label="Switch branch"
              title="Switch branch"
              className="flex size-7 items-center justify-center rounded-md text-slate-500 hover:bg-slate-800 hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60"
            >
              <svg viewBox="0 0 20 20" fill="none" className="size-4" aria-hidden="true">
                <circle cx="5" cy="5" r="2" stroke="currentColor" strokeWidth="1.4" />
                <circle cx="5" cy="15" r="2" stroke="currentColor" strokeWidth="1.4" />
                <circle cx="15" cy="7" r="2" stroke="currentColor" strokeWidth="1.4" />
                <path
                  d="M5 7v6M7 5h4a2 2 0 0 1 2 2v0"
                  stroke="currentColor"
                  strokeWidth="1.4"
                  strokeLinecap="round"
                />
              </svg>
            </button>
          ) : null}
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
  pendingPath,
  onOpenDiff,
  onToggleStage,
}: {
  title: string;
  files: GitFileChange[];
  summaries: Map<string, DiffSummary>;
  staged: boolean;
  untracked?: boolean;
  pendingPath?: string;
  onOpenDiff: (path: string, staged: boolean, includeUntracked: boolean) => void;
  onToggleStage: (path: string) => void;
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
            const pending = pendingPath === file.path;
            return (
              <div
                key={`${file.path}:${staged ? 'staged' : untracked ? 'untracked' : 'worktree'}`}
                className="flex items-center gap-1 rounded-md transition-colors hover:bg-slate-800/60 focus-within:bg-slate-800/60"
              >
                <button
                  type="button"
                  data-testid="git-file"
                  data-path={file.path}
                  onClick={() => onOpenDiff(file.path, staged, untracked)}
                  className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/50"
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
                <button
                  type="button"
                  data-testid={staged ? 'git-unstage' : 'git-stage'}
                  data-path={file.path}
                  aria-label={`${staged ? 'Unstage' : 'Stage'} ${file.path}`}
                  title={staged ? 'Unstage' : 'Stage'}
                  onClick={() => onToggleStage(file.path)}
                  disabled={pending}
                  className={cn(
                    'mr-1 shrink-0 rounded border border-slate-700 px-1.5 py-0.5 text-[10px] font-medium transition-colors',
                    'text-slate-400 hover:border-teal-500/50 hover:text-teal-200',
                    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60',
                    'disabled:cursor-wait disabled:opacity-50',
                  )}
                >
                  {pending ? '…' : staged ? 'Unstage' : 'Stage'}
                </button>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

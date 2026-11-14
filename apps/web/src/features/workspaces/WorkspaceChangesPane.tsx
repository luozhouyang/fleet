import type { DiffSummary, GitStatus } from '@agent-fleet/protocol-types';
import { ArrowDown, ArrowUp, GitBranch, Plus, Undo2 } from 'lucide-react';
import { useMemo, useState } from 'react';
import { ConfirmDialog } from '@/components/dialogs/confirm-dialog';
import { Button } from '@/components/ui/button';
import { EmptyState, LoadingState } from '@/components/ui/feedback';
import { Input, Textarea } from '@/components/ui/input';
import { Select } from '@/components/ui/select';
import { useGitBranches, useGitDiffSummary, useGitLog } from '@/lib/query/hooks';
import { useGitCheckout, useGitCommit, useGitStage, useGitUnstage } from '@/lib/query/mutations';
import { cn } from '@/lib/utils/cn';
import { formatRelativeTime, getErrorMessage, shortId } from '@/lib/utils/format';
import { gitStatusClass, gitStatusLetter } from './workspace-utils';

export function WorkspaceChangesPane({
  workspaceId,
  isGit,
  status,
  statusLoading,
  statusError,
  activePath,
  onOpenDiff,
}: {
  workspaceId: string;
  isGit: boolean;
  status?: GitStatus;
  statusLoading: boolean;
  statusError?: string;
  activePath?: string;
  onOpenDiff: (path: string, staged: boolean) => void;
}) {
  const [message, setMessage] = useState('');
  const [commitResult, setCommitResult] = useState<string | null>(null);
  const [creatingBranch, setCreatingBranch] = useState(false);
  const [branchName, setBranchName] = useState('');
  const [pendingBranch, setPendingBranch] = useState<string | null>(null);

  const summaryQuery = useGitDiffSummary(workspaceId, { include_untracked: true }, isGit);
  const branchesQuery = useGitBranches(workspaceId, isGit);
  const logQuery = useGitLog(workspaceId, undefined, isGit);
  const stageMutation = useGitStage(workspaceId);
  const unstageMutation = useGitUnstage(workspaceId);
  const commitMutation = useGitCommit(workspaceId);
  const checkoutMutation = useGitCheckout(workspaceId);

  const summaryByPath = useMemo(() => {
    const map = new Map<string, DiffSummary>();
    for (const file of summaryQuery.data?.files ?? []) {
      map.set(file.path, file);
    }
    return map;
  }, [summaryQuery.data]);

  const files = status?.files ?? [];
  const stagedFiles = files.filter((file) => file.staged);
  const branches = (branchesQuery.data ?? []).filter((branch) => !branch.is_remote);
  const currentBranch = branches.find((branch) => branch.is_current)?.name ?? status?.branch ?? '';
  const indexPending = stageMutation.isPending || unstageMutation.isPending;
  const fileError = stageMutation.error ?? unstageMutation.error;
  const canCommit =
    stagedFiles.length > 0 && message.trim().length > 0 && !commitMutation.isPending;

  if (!isGit) {
    return (
      <aside className="flex w-[324px] shrink-0 flex-col overflow-hidden rounded-lg border border-border bg-card shadow-card">
        <div className="flex h-10 shrink-0 items-center justify-between border-b border-border px-3">
          <span className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
            CHANGES
          </span>
          <span className="font-mono text-[9px] text-faint">—</span>
        </div>
        <EmptyState
          title="Not a Git repository"
          description="Register a Git repository to review changes, stage files, and commit."
          icon={<GitBranch className="size-4" aria-hidden="true" />}
          className="flex-1"
        />
      </aside>
    );
  }

  const commit = () => {
    if (!canCommit) return;
    setCommitResult(null);
    commitMutation.mutate(
      { message: message.trim() },
      {
        onSuccess: (result) => {
          setMessage('');
          setCommitResult(`Committed ${shortId(result.commit, 7)}`);
        },
      },
    );
  };

  return (
    <aside className="flex w-[324px] shrink-0 flex-col overflow-hidden rounded-lg border border-border bg-card shadow-card">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-border px-3">
        <span className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
          CHANGES
        </span>
        <span className="font-mono text-[9px] text-faint">
          {statusLoading ? '…' : statusError ? '—' : files.length}
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex items-center gap-2 border-b border-border px-3 py-2.5">
          <GitBranch className="size-3 shrink-0 text-faint" aria-hidden="true" />
          <span data-testid="git-branch" className="truncate font-mono text-[10px] text-body">
            {status?.branch ?? (statusLoading ? '…' : 'detached')}
          </span>
          {status?.ahead ? (
            <span
              className="ml-auto inline-flex items-center gap-0.5 font-mono text-[9px] text-success"
              title={`${status.ahead} commits ahead`}
            >
              <ArrowUp className="size-2.5" aria-hidden="true" />
              {status.ahead}
            </span>
          ) : null}
          {status?.behind ? (
            <span
              className={cn(
                'inline-flex items-center gap-0.5 font-mono text-[9px] text-warning',
                !status.ahead && 'ml-auto',
              )}
              title={`${status.behind} commits behind`}
            >
              <ArrowDown className="size-2.5" aria-hidden="true" />
              {status.behind}
            </span>
          ) : null}
        </div>

        <div className="border-b border-border px-3 py-3">
          <label
            htmlFor="workspace-branch-select"
            className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
          >
            Switch branch
          </label>
          <Select
            id="workspace-branch-select"
            value={currentBranch}
            onValueChange={(value) => {
              if (!value || value === currentBranch) return;
              checkoutMutation.reset();
              setPendingBranch(value);
            }}
            options={branches.map((branch) => ({
              value: branch.name,
              label: branch.name,
              description: branch.upstream,
            }))}
            placeholder={branchesQuery.isLoading ? 'Loading branches…' : 'No branches'}
            aria-label="Switch branch"
            disabled={checkoutMutation.isPending || branches.length === 0}
            className="mt-1.5 h-8 text-[11.5px]"
          />
          {checkoutMutation.isError ? (
            <p className="mt-1.5 text-[10.5px] leading-4 text-destructive">
              {getErrorMessage(checkoutMutation.error)}
            </p>
          ) : null}

          <div className="mt-2">
            {creatingBranch ? (
              <form
                className="flex items-center gap-1.5"
                onSubmit={(event) => {
                  event.preventDefault();
                  const name = branchName.trim();
                  if (!name) return;
                  checkoutMutation.mutate(
                    { branch: name, create: true },
                    {
                      onSuccess: () => {
                        setCreatingBranch(false);
                        setBranchName('');
                      },
                    },
                  );
                }}
              >
                <Input
                  autoFocus
                  value={branchName}
                  onChange={(event) => setBranchName(event.target.value)}
                  placeholder="feature/my-branch"
                  aria-label="New branch name"
                  data-testid="git-new-branch-input"
                  spellCheck={false}
                  className="h-8 flex-1 font-mono text-[11px]"
                />
                <Button
                  type="submit"
                  size="sm"
                  variant="secondary"
                  disabled={!branchName.trim() || checkoutMutation.isPending}
                  data-testid="git-new-branch-submit"
                >
                  Create
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setCreatingBranch(false);
                    setBranchName('');
                  }}
                >
                  Cancel
                </Button>
              </form>
            ) : (
              <button
                type="button"
                onClick={() => setCreatingBranch(true)}
                data-testid="git-new-branch"
                className="flex items-center gap-1 rounded-sm py-1 font-mono text-[9.5px] text-muted-foreground transition-colors hover:text-foreground"
              >
                <Plus className="size-3" aria-hidden="true" />
                New branch
              </button>
            )}
          </div>
        </div>

        <div className="border-b border-border py-1">
          {statusLoading ? (
            <LoadingState label="Loading changes…" className="py-6" />
          ) : statusError ? (
            <p className="px-3 py-3 text-[10.5px] leading-4 text-destructive">{statusError}</p>
          ) : files.length === 0 ? (
            <p className="px-3 py-4 text-center font-mono text-[9.5px] text-faint">
              Working tree clean
            </p>
          ) : (
            files.map((file) => {
              const summary = summaryByPath.get(file.path);
              const selected = activePath === file.path;
              return (
                <div
                  key={`${file.path}:${file.staged}`}
                  className={cn(
                    'group flex items-center gap-2 px-3 py-1 transition-colors hover:bg-muted',
                    selected && 'bg-selected',
                  )}
                >
                  <span
                    className={cn(
                      'w-2 shrink-0 text-center font-mono text-[10px] font-bold',
                      gitStatusClass(file.status),
                    )}
                    title={file.status}
                  >
                    {gitStatusLetter(file.status)}
                  </span>
                  <button
                    type="button"
                    onClick={() => onOpenDiff(file.path, file.staged)}
                    data-testid="git-file"
                    data-path={file.path}
                    title={file.old_path ? `${file.old_path} → ${file.path}` : file.path}
                    className="flex min-w-0 flex-1 items-baseline gap-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/35"
                  >
                    <span className="truncate font-mono text-[10.5px] text-body">{file.path}</span>
                    {summary && (summary.additions > 0 || summary.deletions > 0) ? (
                      <span className="ml-auto flex shrink-0 items-baseline gap-1 font-mono text-[9px]">
                        {summary.additions > 0 ? (
                          <span className="text-success">+{summary.additions}</span>
                        ) : null}
                        {summary.deletions > 0 ? (
                          <span className="text-destructive">-{summary.deletions}</span>
                        ) : null}
                      </span>
                    ) : null}
                  </button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={indexPending}
                    onClick={() =>
                      file.staged
                        ? unstageMutation.mutate([file.path])
                        : stageMutation.mutate([file.path])
                    }
                    aria-label={file.staged ? `Unstage ${file.path}` : `Stage ${file.path}`}
                    data-testid={file.staged ? 'git-unstage' : 'git-stage'}
                    data-path={file.path}
                    title={file.staged ? 'Unstage file' : 'Stage file'}
                    className={cn(
                      'shrink-0 text-faint',
                      !file.staged && 'opacity-60 group-hover:opacity-100',
                    )}
                  >
                    {file.staged ? (
                      <Undo2 className="size-3" aria-hidden="true" />
                    ) : (
                      <Plus className="size-3" aria-hidden="true" />
                    )}
                  </Button>
                </div>
              );
            })
          )}
          {fileError ? (
            <p className="px-3 py-2 text-[10.5px] leading-4 text-destructive">
              {getErrorMessage(fileError)}
            </p>
          ) : null}
        </div>

        <div className="border-b border-border px-3 py-3">
          <div className="flex items-center justify-between gap-2">
            <label
              htmlFor="workspace-commit-message"
              className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint"
            >
              Commit message
            </label>
            <span className="font-mono text-[9px] text-faint">{stagedFiles.length} staged</span>
          </div>
          <Textarea
            id="workspace-commit-message"
            data-testid="git-commit-message"
            value={message}
            onChange={(event) => setMessage(event.target.value)}
            placeholder="Describe the change…"
            disabled={commitMutation.isPending}
            className="mt-1.5 min-h-[64px] text-[11.5px]"
          />
          <Button
            variant="primary"
            className="mt-2 w-full"
            data-testid="git-commit-submit"
            disabled={!canCommit}
            onClick={commit}
          >
            {commitMutation.isPending
              ? 'Committing…'
              : `Commit ${stagedFiles.length} ${stagedFiles.length === 1 ? 'file' : 'files'}`}
          </Button>
          {commitMutation.isError ? (
            <p className="mt-1.5 text-[10.5px] leading-4 text-destructive">
              {getErrorMessage(commitMutation.error)}
            </p>
          ) : null}
          {commitResult ? (
            <p className="mt-1.5 text-[10.5px] leading-4 text-success">{commitResult}</p>
          ) : null}
        </div>

        <div className="px-3 py-3">
          <p className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
            Recent commits
          </p>
          {logQuery.isLoading ? (
            <p className="mt-1.5 font-mono text-[9px] text-faint">Loading…</p>
          ) : logQuery.isError ? (
            <p className="mt-1.5 text-[10.5px] leading-4 text-destructive">
              {getErrorMessage(logQuery.error)}
            </p>
          ) : (logQuery.data ?? []).length === 0 ? (
            <p className="mt-1.5 font-mono text-[9px] text-faint">No commits yet</p>
          ) : (
            (logQuery.data ?? []).slice(0, 5).map((entry) => (
              <div key={entry.id} className="flex items-baseline gap-2 py-1">
                <span className="shrink-0 font-mono text-[9px] text-primary">{entry.short_id}</span>
                <span className="min-w-0 flex-1 truncate text-[11.5px] text-body">
                  {entry.message}
                </span>
                <span className="shrink-0 font-mono text-[8.5px] text-faint">
                  {formatRelativeTime(entry.timestamp)}
                </span>
              </div>
            ))
          )}
        </div>
      </div>

      <ConfirmDialog
        open={pendingBranch !== null}
        title="Switch branch?"
        description={
          pendingBranch
            ? `Check out ${pendingBranch}. Uncommitted changes stay in the working tree.`
            : undefined
        }
        confirmLabel="Switch branch"
        pending={checkoutMutation.isPending}
        error={checkoutMutation.error ? getErrorMessage(checkoutMutation.error) : undefined}
        onConfirm={() => {
          if (!pendingBranch) return;
          checkoutMutation.mutate(
            { branch: pendingBranch },
            { onSuccess: () => setPendingBranch(null) },
          );
        }}
        onOpenChange={(next) => {
          if (!next && !checkoutMutation.isPending) {
            setPendingBranch(null);
            checkoutMutation.reset();
          }
        }}
      />
    </aside>
  );
}

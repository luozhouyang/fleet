import type { SessionStatus } from '@agent-fleet/protocol-types';
import { GitBranch, ShieldAlert } from 'lucide-react';
import { useGitStatus } from '@/lib/query/hooks';
import { useSessionStore } from '@/lib/session-store';
import {
  activityLabel,
  activityTone,
  branchLabel,
  pendingPromptCount,
  usageLabel,
} from '@/lib/session/session-header';
import { cn } from '@/lib/utils/cn';

const ACTIVITY_TONE: Record<string, string> = {
  running: 'text-status-running',
  waiting: 'text-status-attention',
  idle: 'text-faint',
};

/**
 * Secondary header facts: branch, live usage, background activity and the
 * number of prompts blocking the agent.
 *
 * These are live-only values (except the pending prompts), so they simply
 * disappear on reload rather than being persisted.
 */
export function SessionHeaderChips({
  sessionId,
  workspaceId,
  isGitRepository,
  pendingPrompts,
  status,
}: {
  sessionId: string;
  workspaceId?: string;
  isGitRepository?: boolean;
  pendingPrompts: number;
  status: SessionStatus | undefined;
}) {
  const activity = useSessionStore(
    (state) => state.sessions[sessionId]?.activity ?? null,
  );
  const usage = useSessionStore((state) => state.sessions[sessionId]?.usage ?? null);
  const gitStatus = useGitStatus(workspaceId ?? '', Boolean(workspaceId) && isGitRepository);

  const branch = branchLabel(gitStatus.data?.branch);
  const usageText = usageLabel(usage);
  const activityText = activityLabel(activity, status);
  const tone = activityTone(activity, status);

  if (!branch && !usageText && !activityText && pendingPrompts === 0) return null;

  return (
    <div className="mt-1 flex min-w-0 flex-wrap items-center gap-1.5">
      {branch ? (
        <span
          className="inline-flex max-w-[220px] items-center gap-1 rounded border border-border bg-card px-1.5 py-0.5 font-mono text-[9px] text-muted-foreground"
          title={gitStatus.data?.branch}
        >
          <GitBranch className="size-2.5 shrink-0" aria-hidden="true" />
          <span className="truncate">{branch}</span>
          {gitStatus.data && (gitStatus.data.ahead > 0 || gitStatus.data.behind > 0) ? (
            <span className="shrink-0 text-faint">
              {gitStatus.data.ahead > 0 ? `↑${gitStatus.data.ahead}` : ''}
              {gitStatus.data.behind > 0 ? `↓${gitStatus.data.behind}` : ''}
            </span>
          ) : null}
        </span>
      ) : null}

      {activityText ? (
        <span
          className={cn(
            'inline-flex items-center gap-1 rounded border border-border bg-card px-1.5 py-0.5 font-mono text-[9px]',
            tone ? ACTIVITY_TONE[tone] : 'text-muted-foreground',
          )}
        >
          <span
            aria-hidden="true"
            className={cn(
              'size-1 rounded-full',
              tone === 'running' && 'animate-pulse-dot bg-status-running',
              tone === 'waiting' && 'bg-status-attention',
              tone === 'idle' && 'bg-faint',
            )}
          />
          <span className="truncate">{activityText}</span>
        </span>
      ) : null}

      {usageText ? (
        <span className="inline-flex items-center rounded border border-border bg-card px-1.5 py-0.5 font-mono text-[9px] text-muted-foreground">
          {usageText}
        </span>
      ) : null}

      {pendingPrompts > 0 ? (
        <span className="inline-flex items-center gap-1 rounded border border-warning/30 bg-warning-soft px-1.5 py-0.5 font-mono text-[9px] font-bold text-warning">
          <ShieldAlert className="size-2.5" aria-hidden="true" />
          {pendingPromptCount(pendingPrompts, 0)} waiting
        </span>
      ) : null}
    </div>
  );
}

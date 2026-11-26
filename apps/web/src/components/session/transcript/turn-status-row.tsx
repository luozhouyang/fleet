import { TurnStatus, type AgentTurn } from '@agent-fleet/protocol-types';
import { CircleDot, CirclePause, CircleX, Loader, Check, type LucideIcon } from 'lucide-react';
import { memo, useEffect, useState } from 'react';
import { useSessionStore } from '@/lib/session-store';
import { cn } from '@/lib/utils/cn';
import { formatDuration } from '@/lib/utils/format';

const STATUS: Record<
  TurnStatus,
  { icon: LucideIcon; label: string; tone: string; spin?: boolean }
> = {
  [TurnStatus.Running]: {
    icon: Loader,
    label: 'Working',
    tone: 'text-status-running',
    spin: true,
  },
  [TurnStatus.Completed]: { icon: Check, label: 'Completed', tone: 'text-muted-foreground' },
  [TurnStatus.Interrupted]: {
    icon: CirclePause,
    label: 'Interrupted',
    tone: 'text-muted-foreground',
  },
  [TurnStatus.Failed]: { icon: CircleX, label: 'Failed', tone: 'text-status-failed' },
  [TurnStatus.Pending]: { icon: CircleDot, label: 'Queued', tone: 'text-faint' },
};

/** Re-render once a second while a turn is running, for its elapsed label. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const handle = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(handle);
  }, [active]);
  return now;
}

function elapsedLabel(turn: AgentTurn, now: number): string | null {
  const started = Date.parse(turn.started_at);
  if (Number.isNaN(started)) return null;
  const ended = turn.completed_at ? Date.parse(turn.completed_at) : now;
  if (Number.isNaN(ended) || ended < started) return null;
  return formatDuration((ended - started) / 1000);
}

/**
 * Closes a turn with its outcome and duration. A running turn shows an
 * elapsed clock so a long tool call never looks stuck.
 */
export const TurnStatusRow = memo(function TurnStatusRow({
  sessionId,
  turnId,
  fallback,
}: {
  sessionId: string;
  turnId: string | null;
  fallback: AgentTurn | null;
}) {
  const liveTurn = useSessionStore((state) =>
    turnId ? state.sessions[sessionId]?.turns[turnId] : undefined,
  );
  const turn = liveTurn ?? fallback;
  const running = turn?.status === TurnStatus.Running;
  const now = useNow(running);
  if (!turn) return null;

  const meta = STATUS[turn.status] ?? STATUS[TurnStatus.Completed];
  const Icon = meta.icon;
  const elapsed = elapsedLabel(turn, now);
  const errorMessage = turn.error?.message?.trim();

  return (
    <div className="pb-5" data-testid="turn-status" data-turn-id={turn.id}>
      <div className="flex items-center gap-2">
        <span className="h-px flex-1 bg-border" aria-hidden="true" />
        {/* Only the status word is announced. The elapsed clock ticks every
            second, and a live region around it would read the whole line each
            time. */}
        <span
          aria-live={running ? 'polite' : 'off'}
          className={cn('flex items-center gap-1.5 font-mono text-[9.5px]', meta.tone)}
        >
          <Icon
            className={cn('size-3', meta.spin && 'animate-spin motion-reduce:animate-none')}
            aria-hidden="true"
          />
          {meta.label}
        </span>
        {elapsed ? (
          <span aria-hidden="true" className="font-mono text-[9.5px] text-faint">
            · {elapsed}
          </span>
        ) : null}
        <span className="h-px flex-1 bg-border" aria-hidden="true" />
      </div>
      {errorMessage ? (
        <p
          className="mt-1.5 truncate text-center text-[10.5px] text-destructive"
          title={errorMessage}
        >
          {errorMessage}
        </p>
      ) : null}
    </div>
  );
});

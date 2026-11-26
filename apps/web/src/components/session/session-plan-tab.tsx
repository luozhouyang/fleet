import type { PlanEntryStatus } from '@agent-fleet/protocol-types';
import { Circle, CircleCheck, CircleDot, CircleSlash, CircleX, Loader } from 'lucide-react';
import { EmptyState } from '@/components/ui/feedback';
import { useSessionStore } from '@/lib/session-store';
import { cn } from '@/lib/utils/cn';
import { formatRelativeTime } from '@/lib/utils/format';

const ENTRY: Record<PlanEntryStatus, { icon: typeof Circle; tone: string; label: string }> = {
  pending: { icon: Circle, tone: 'text-faint', label: 'Pending' },
  in_progress: { icon: Loader, tone: 'text-status-running', label: 'In progress' },
  completed: { icon: CircleCheck, tone: 'text-status-running', label: 'Completed' },
  failed: { icon: CircleX, tone: 'text-status-failed', label: 'Failed' },
  cancelled: { icon: CircleSlash, tone: 'text-faint', label: 'Cancelled' },
  unknown: { icon: CircleDot, tone: 'text-faint', label: 'Unknown' },
};

/** The agent's current plan, shown in the context panel. */
export function SessionPlanTab({ sessionId }: { sessionId: string }) {
  const plan = useSessionStore((state) => state.sessions[sessionId]?.plan ?? null);

  if (!plan || plan.entries.length === 0) {
    return (
      <EmptyState
        title="No plan yet"
        description="When the agent publishes a plan, its steps appear here and update in place."
        className="flex-1"
      />
    );
  }

  const completed = plan.entries.filter((entry) => entry.status === 'completed').length;

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-5 pt-3">
      <div className="flex items-center justify-between gap-2">
        <p className="font-mono text-[9px] uppercase tracking-[0.12em] text-faint">Plan</p>
        <p className="font-mono text-[9px] text-muted-foreground">
          {completed}/{plan.entries.length} · {formatRelativeTime(plan.updated_at)}
        </p>
      </div>
      <ol className="mt-2 flex flex-col">
        {plan.entries.map((entry, index) => {
          const meta = ENTRY[entry.status] ?? ENTRY.unknown;
          const Icon = meta.icon;
          return (
            <li
              key={entry.id ?? index}
              className="flex items-start gap-2 border-b border-border py-2 last:border-0"
            >
              <Icon
                className={cn(
                  'mt-0.5 size-3.5 shrink-0',
                  meta.tone,
                  entry.status === 'in_progress' && 'animate-spin motion-reduce:animate-none',
                )}
                aria-label={meta.label}
              />
              <span
                className={cn(
                  'min-w-0 flex-1 break-words text-[11.5px] leading-[17px]',
                  entry.status === 'completed' ? 'text-muted-foreground line-through' : 'text-body',
                )}
              >
                {entry.content}
              </span>
              {entry.priority === 'high' ? (
                <span className="shrink-0 font-mono text-[8.5px] uppercase text-status-attention">
                  high
                </span>
              ) : null}
            </li>
          );
        })}
      </ol>
    </div>
  );
}

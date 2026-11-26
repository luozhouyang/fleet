import type { AgentPlan, PlanEntryStatus } from '@agent-fleet/protocol-types';
import {
  Circle,
  CircleCheck,
  CircleDot,
  CircleSlash,
  CircleX,
  ListTodo,
  Loader,
  type LucideIcon,
} from 'lucide-react';
import { memo } from 'react';
import { cn } from '@/lib/utils/cn';
import { formatRelativeTime } from '@/lib/utils/format';

const ENTRY_STATUS: Record<
  PlanEntryStatus,
  { icon: LucideIcon; tone: string; spin?: boolean; label: string }
> = {
  pending: { icon: Circle, tone: 'text-faint', label: 'Pending' },
  in_progress: {
    icon: Loader,
    tone: 'text-status-running',
    spin: true,
    label: 'In progress',
  },
  completed: { icon: CircleCheck, tone: 'text-status-running', label: 'Completed' },
  failed: { icon: CircleX, tone: 'text-status-failed', label: 'Failed' },
  cancelled: { icon: CircleSlash, tone: 'text-faint', label: 'Cancelled' },
  unknown: { icon: CircleDot, tone: 'text-faint', label: 'Unknown' },
};

/**
 * The agent's current plan. Plans replace each other, so this renders the
 * latest one at the point where it was last updated.
 */
export const PlanRow = memo(function PlanRow({ plan }: { plan: AgentPlan }) {
  const completed = plan.entries.filter((entry) => entry.status === 'completed').length;
  return (
    <div
      data-testid="plan-card"
      className="mb-4 rounded-lg border border-border bg-card p-3 shadow-card-sm"
    >
      <div className="flex items-center gap-2">
        <ListTodo className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
        <span className="text-[11px] font-bold uppercase tracking-[0.12em] text-faint">Plan</span>
        <span className="font-mono text-[9.5px] text-muted-foreground">
          {completed}/{plan.entries.length}
        </span>
        <span className="ml-auto shrink-0 font-mono text-[9px] text-faint">
          {formatRelativeTime(plan.updated_at)}
        </span>
      </div>
      <ol className="mt-2 flex flex-col gap-1.5">
        {plan.entries.map((entry, index) => {
          const meta = ENTRY_STATUS[entry.status] ?? ENTRY_STATUS.unknown;
          const Icon = meta.icon;
          return (
            <li
              key={entry.id ?? `${index}`}
              className="flex items-start gap-2 text-[11.5px] leading-[17px]"
            >
              <Icon
                className={cn(
                  'mt-[1px] size-3 shrink-0',
                  meta.tone,
                  meta.spin && 'animate-spin motion-reduce:animate-none',
                )}
                aria-label={meta.label}
              />
              <span
                className={cn(
                  'min-w-0 flex-1 break-words',
                  entry.status === 'completed' ? 'text-muted-foreground line-through' : 'text-body',
                )}
              >
                {entry.content}
              </span>
            </li>
          );
        })}
      </ol>
    </div>
  );
});

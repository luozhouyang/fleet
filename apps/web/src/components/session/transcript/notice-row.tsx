import { CircleX, Info, TriangleAlert, type LucideIcon } from 'lucide-react';
import { memo } from 'react';
import type { SessionNotice } from '@/lib/session-reducer';
import { cn } from '@/lib/utils/cn';
import { formatClock } from '@/lib/utils/format';

const LEVELS: Record<string, { icon: LucideIcon; tone: string }> = {
  info: { icon: Info, tone: 'text-muted-foreground' },
  warning: { icon: TriangleAlert, tone: 'text-warning' },
  error: { icon: CircleX, tone: 'text-destructive' },
};

/**
 * A durable agent notice.
 *
 * No live region here: the transcript is a `role="log"`, which is already a
 * polite live region, so a nested `role="status"` would announce every notice
 * twice.
 */
export const NoticeRow = memo(function NoticeRow({ notice }: { notice: SessionNotice }) {
  const level = LEVELS[notice.level] ?? LEVELS.info;
  const Icon = level.icon;
  return (
    <div className="flex items-start gap-2 pb-4 text-[11px] leading-[17px]">
      <Icon className={cn('mt-0.5 size-3 shrink-0', level.tone)} aria-hidden="true" />
      <span className="min-w-0 flex-1 whitespace-pre-wrap break-words text-muted-foreground">
        {notice.message}
      </span>
      <span className="shrink-0 font-mono text-[9px] text-faint">{formatClock(notice.at)}</span>
    </div>
  );
});

import type { StatusTone } from '@/lib/status';
import { cn } from '@/lib/utils/cn';

const DOT: Record<StatusTone, string> = {
  running: 'bg-success',
  waiting: 'bg-warning',
  failed: 'bg-destructive',
  idle: 'bg-faint',
  offline: 'bg-border-strong',
};

const TEXT: Record<StatusTone, string> = {
  running: 'text-success',
  waiting: 'text-warning',
  failed: 'text-destructive',
  idle: 'text-muted-foreground',
  offline: 'text-faint',
};

const SIZE = {
  xs: 'size-1',
  sm: 'size-1.5',
  md: 'size-2',
};

export interface StatusDotProps {
  tone: StatusTone;
  label?: string;
  size?: keyof typeof SIZE;
  pulse?: boolean;
  className?: string;
  labelClassName?: string;
}

export function StatusDot({
  tone,
  label,
  size = 'sm',
  pulse = false,
  className,
  labelClassName,
}: StatusDotProps) {
  return (
    <span className={cn('inline-flex items-center gap-1.5', className)}>
      <span
        aria-hidden="true"
        className={cn(
          'shrink-0 rounded-full',
          SIZE[size],
          DOT[tone],
          pulse && tone === 'running' && 'animate-pulse-dot',
        )}
      />
      {label ? (
        <span
          className={cn('truncate text-[9.5px] font-bold leading-4', TEXT[tone], labelClassName)}
        >
          {label}
        </span>
      ) : null}
    </span>
  );
}

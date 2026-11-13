import type { HTMLAttributes, ReactNode } from 'react';
import { cn } from '../cn';

export type Tone = 'neutral' | 'slate' | 'teal' | 'green' | 'amber' | 'red';

const badgeTones: Record<Tone, string> = {
  neutral: 'border-slate-700/80 bg-slate-800/60 text-slate-300',
  slate: 'border-slate-600/60 bg-slate-800/40 text-slate-400',
  teal: 'border-teal-500/30 bg-teal-500/10 text-teal-300',
  green: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300',
  amber: 'border-amber-500/30 bg-amber-500/10 text-amber-300',
  red: 'border-red-500/30 bg-red-500/10 text-red-300',
};

const dotTones: Record<Tone, string> = {
  neutral: 'bg-slate-400',
  slate: 'bg-slate-500',
  teal: 'bg-teal-400',
  green: 'bg-emerald-400',
  amber: 'bg-amber-400',
  red: 'bg-red-400',
};

export function Badge({
  tone = 'neutral',
  className,
  children,
  ...props
}: HTMLAttributes<HTMLSpanElement> & { tone?: Tone; children: ReactNode }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[11px] font-medium leading-5',
        badgeTones[tone],
        className,
      )}
      {...props}
    >
      {children}
    </span>
  );
}

export function StatusDot({
  tone = 'neutral',
  pulse = false,
  className,
}: {
  tone?: Tone;
  pulse?: boolean;
  className?: string;
}) {
  return (
    <span className={cn('relative inline-flex size-2', className)} aria-hidden="true">
      {pulse ? (
        <span
          className={cn('absolute inline-flex size-full animate-ping rounded-full opacity-60', dotTones[tone])}
        />
      ) : null}
      <span className={cn('relative inline-flex size-2 rounded-full', dotTones[tone])} />
    </span>
  );
}

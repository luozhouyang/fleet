import type { ReactNode } from 'react';
import { cn } from '@/lib/utils/cn';

export function PageKicker({ children }: { children: ReactNode }) {
  return (
    <div className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
      {children}
    </div>
  );
}

export function Panel({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <section
      className={cn(
        'overflow-hidden rounded-lg border border-border bg-card shadow-card',
        className,
      )}
    >
      {children}
    </section>
  );
}

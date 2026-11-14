import type { ReactNode } from 'react';
import { cn } from '@/lib/utils/cn';

export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <kbd
      className={cn(
        'inline-flex h-5 items-center rounded border border-border bg-card px-1.5 font-mono text-[9.5px] text-muted-foreground',
        className,
      )}
    >
      {children}
    </kbd>
  );
}

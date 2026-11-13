import type { HTMLAttributes, ReactNode } from 'react';
import { cn } from '../cn';

export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        'rounded-xl border border-slate-800/90 bg-slate-900/65 shadow-lg shadow-slate-950/20',
        className,
      )}
      {...props}
    />
  );
}

export function CardHeader({
  title,
  description,
  action,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn('flex items-start justify-between gap-3 border-b border-slate-800/80 p-4', className)}>
      <div className="min-w-0">
        <h2 className="truncate text-sm font-semibold text-slate-100">{title}</h2>
        {description ? <p className="mt-1 text-xs text-slate-400">{description}</p> : null}
      </div>
      {action}
    </div>
  );
}

export function CardContent({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('p-4', className)} {...props} />;
}

export function CardFooter({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn('border-t border-slate-800/80 px-4 py-3', className)}
      {...props}
    />
  );
}

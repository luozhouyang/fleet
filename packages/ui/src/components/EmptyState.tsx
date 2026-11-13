import type { ReactNode } from 'react';
import { cn } from '../cn';

export function EmptyState({
  title,
  description,
  action,
  icon,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  icon?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center gap-3 px-6 py-12 text-center',
        className,
      )}
    >
      {icon ? (
        <div className="flex size-11 items-center justify-center rounded-xl border border-slate-800 bg-slate-900 text-slate-500">
          {icon}
        </div>
      ) : null}
      <div className="space-y-1">
        <h3 className="text-sm font-medium text-slate-200">{title}</h3>
        {description ? (
          <p className="mx-auto max-w-md text-xs leading-5 text-slate-400">{description}</p>
        ) : null}
      </div>
      {action}
    </div>
  );
}

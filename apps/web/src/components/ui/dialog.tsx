import { Dialog as BaseDialog } from '@base-ui-components/react/dialog';
import type { ReactNode } from 'react';
import { cn } from '@/lib/utils/cn';

export const Dialog = BaseDialog.Root;
export const DialogTrigger = BaseDialog.Trigger;
export const DialogClose = BaseDialog.Close;
export const DialogTitle = BaseDialog.Title;
export const DialogDescription = BaseDialog.Description;

export function DialogContent({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <BaseDialog.Portal>
      <BaseDialog.Backdrop className="fixed inset-0 z-50 animate-fade bg-overlay/45" />
      <BaseDialog.Popup
        className={cn(
          'fixed left-1/2 top-1/2 z-50 w-full max-w-[560px] -translate-x-1/2 -translate-y-1/2 animate-rise rounded-2xl border border-border bg-card p-6 text-body shadow-modal outline-none',
          className,
        )}
      >
        {children}
      </BaseDialog.Popup>
    </BaseDialog.Portal>
  );
}

export function DialogHeader({
  title,
  description,
  icon,
  eyebrow,
  iconClassName,
  onClose,
}: {
  title: string;
  description?: string;
  icon?: ReactNode;
  eyebrow?: string;
  iconClassName?: string;
  onClose?: () => void;
}) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="flex min-w-0 items-start gap-3">
        {icon ? (
          <span
            className={cn(
              'flex size-10 shrink-0 items-center justify-center rounded-xl bg-primary-soft text-primary',
              iconClassName,
            )}
          >
            {icon}
          </span>
        ) : null}
        <div className="min-w-0">
          {eyebrow ? (
            <p className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
              {eyebrow}
            </p>
          ) : null}
          <DialogTitle className="font-display text-[21px] font-bold leading-7 tracking-[-0.02em] text-foreground">
            {title}
          </DialogTitle>
          {description ? (
            <DialogDescription className="mt-1 text-[12.5px] leading-5 text-muted-foreground">
              {description}
            </DialogDescription>
          ) : null}
        </div>
      </div>
      {onClose ? (
        <BaseDialog.Close
          aria-label="Close dialog"
          className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          onClick={onClose}
        >
          <svg viewBox="0 0 24 24" className="size-4" fill="none" aria-hidden="true">
            <path
              d="M18 6 6 18M6 6l12 12"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
            />
          </svg>
        </BaseDialog.Close>
      ) : null}
    </div>
  );
}

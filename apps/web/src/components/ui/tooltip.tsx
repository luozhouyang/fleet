import { Tooltip as BaseTooltip } from '@base-ui-components/react/tooltip';
import type { ReactElement, ReactNode } from 'react';
import { cn } from '@/lib/utils/cn';

/**
 * A short hover/focus hint.
 *
 * A tooltip is supplementary: the trigger must already have an accessible name
 * (`aria-label` or visible text), because assistive technology is not required
 * to expose the popup.
 */
export function Tooltip({
  content,
  children,
  side = 'top',
  delay = 300,
  className,
}: {
  content: ReactNode;
  children: ReactElement;
  side?: 'top' | 'bottom' | 'left' | 'right';
  delay?: number;
  className?: string;
}) {
  if (content === null || content === undefined || content === '') return children;

  return (
    <BaseTooltip.Root>
      <BaseTooltip.Trigger
        delay={delay}
        render={children as ReactElement<Record<string, unknown>>}
      />
      <BaseTooltip.Portal>
        <BaseTooltip.Positioner side={side} sideOffset={6} className="z-[60]">
          <BaseTooltip.Popup
            className={cn(
              'max-w-[280px] animate-fade rounded-md border border-border bg-card px-2 py-1 text-[11px] leading-4 text-body shadow-floating',
              className,
            )}
          >
            {content}
          </BaseTooltip.Popup>
        </BaseTooltip.Positioner>
      </BaseTooltip.Portal>
    </BaseTooltip.Root>
  );
}

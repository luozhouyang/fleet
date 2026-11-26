import { ContextMenu as BaseContextMenu } from '@base-ui-components/react/context-menu';
import type { ReactNode } from 'react';
import { cn } from '@/lib/utils/cn';

export const ContextMenu = BaseContextMenu.Root;
export const ContextMenuTrigger = BaseContextMenu.Trigger;

/** The pointer-anchored menu surface, matching the dropdown menu's styling. */
export function ContextMenuContent({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <BaseContextMenu.Portal>
      <BaseContextMenu.Positioner className="z-[60]">
        <BaseContextMenu.Popup
          className={cn(
            'min-w-[180px] animate-rise rounded-lg border border-border bg-card p-1 text-[12.5px] text-body shadow-floating outline-none',
            className,
          )}
        >
          {children}
        </BaseContextMenu.Popup>
      </BaseContextMenu.Positioner>
    </BaseContextMenu.Portal>
  );
}

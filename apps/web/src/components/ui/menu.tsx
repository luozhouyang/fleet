import { Menu as BaseMenu } from '@base-ui-components/react/menu';
import type { ReactElement, ReactNode } from 'react';
import { cn } from '@/lib/utils/cn';

export const Menu = BaseMenu.Root;
export const MenuTrigger = BaseMenu.Trigger;

/** The popup plus its positioner and portal, with the shared surface styles. */
export function MenuContent({
  children,
  className,
  align = 'start',
  sideOffset = 4,
}: {
  children: ReactNode;
  className?: string;
  align?: 'start' | 'center' | 'end';
  sideOffset?: number;
}) {
  return (
    <BaseMenu.Portal>
      <BaseMenu.Positioner sideOffset={sideOffset} align={align} className="z-[60]">
        <BaseMenu.Popup
          className={cn(
            'min-w-[180px] animate-rise rounded-lg border border-border bg-card p-1 text-[12.5px] text-body shadow-floating outline-none',
            className,
          )}
        >
          {children}
        </BaseMenu.Popup>
      </BaseMenu.Positioner>
    </BaseMenu.Portal>
  );
}

export function MenuItem({
  children,
  icon,
  shortcut,
  danger,
  disabled,
  onClick,
  className,
}: {
  children: ReactNode;
  icon?: ReactNode;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  className?: string;
}) {
  return (
    <BaseMenu.Item
      disabled={disabled}
      onClick={onClick}
      className={cn(
        'flex cursor-default select-none items-center gap-2 rounded-sm px-2 py-1.5 outline-none data-[disabled]:pointer-events-none data-[disabled]:opacity-45 data-[highlighted]:bg-muted',
        danger ? 'text-destructive data-[highlighted]:bg-destructive-soft' : 'text-body',
        className,
      )}
    >
      {icon ? (
        <span className="flex size-3.5 shrink-0 items-center justify-center" aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {shortcut ? (
        <span className="ml-2 shrink-0 font-mono text-[10px] text-faint">{shortcut}</span>
      ) : null}
    </BaseMenu.Item>
  );
}

export function MenuSeparator() {
  return <BaseMenu.Separator className="my-1 h-px bg-border" />;
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return (
    <BaseMenu.GroupLabel className="px-2 py-1 font-mono text-[9px] uppercase tracking-[0.08em] text-faint">
      {children}
    </BaseMenu.GroupLabel>
  );
}

/** Convenience wrapper: a trigger rendered from `trigger` and a menu body. */
export function IconMenu({
  trigger,
  children,
  align = 'end',
  label,
}: {
  trigger: ReactNode;
  children: ReactNode;
  align?: 'start' | 'center' | 'end';
  label: string;
}) {
  return (
    <Menu>
      <MenuTrigger
        aria-label={label}
        render={trigger as ReactElement<Record<string, unknown>>}
        className="outline-none"
      />
      <MenuContent align={align}>{children}</MenuContent>
    </Menu>
  );
}

import { Tabs as BaseTabs } from '@base-ui-components/react/tabs';
import { cn } from '@/lib/utils/cn';

export const Tabs = BaseTabs.Root;
export const TabsPanel = BaseTabs.Panel;

export function TabsList({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <BaseTabs.List className={cn('flex items-end gap-5 border-b border-border', className)}>
      {children}
    </BaseTabs.List>
  );
}

export function TabsTrigger({
  value,
  className,
  children,
}: {
  value: string | number;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <BaseTabs.Tab
      value={value}
      className={cn(
        '-mb-px flex h-10 items-center gap-1.5 rounded-sm border-b-2 border-transparent px-0.5 text-[11.5px] font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40 data-[selected]:border-primary data-[selected]:text-foreground',
        className,
      )}
    >
      {children}
    </BaseTabs.Tab>
  );
}

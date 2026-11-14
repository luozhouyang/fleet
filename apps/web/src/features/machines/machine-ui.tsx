import { MachineStatus, type AgentCapabilities } from '@agent-fleet/protocol-types';
import { Fragment, type ReactNode } from 'react';
import { StatusDot } from '@/components/ui/status-dot';
import { machineStatusMeta } from '@/lib/status';
import { cn } from '@/lib/utils/cn';

/** Shared page frame for the Machines index and detail layouts. */
export const pageContainer =
  'mx-auto flex w-full max-w-[1440px] flex-col gap-5 px-5 pb-10 pt-4 sm:px-8 lg:px-10 lg:pb-12';

const ONLINE_STATUSES = new Set<MachineStatus>([MachineStatus.Online, MachineStatus.Degraded]);

export function machineIsOnline(status?: MachineStatus | null): boolean {
  return status != null && ONLINE_STATUSES.has(status);
}

export function PageKicker({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        'font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint',
        className,
      )}
    >
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

export function PanelHeading({
  title,
  trailing,
  className,
}: {
  title: string;
  trailing?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'flex items-center justify-between gap-3 border-b border-border px-5 py-3',
        className,
      )}
    >
      <h2 className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
        {title}
      </h2>
      {trailing ? <div className="shrink-0">{trailing}</div> : null}
    </div>
  );
}

export function MachineStatusIndicator({
  status,
  showLabel = true,
  size = 'sm',
  className,
  labelClassName,
}: {
  status?: MachineStatus | null;
  showLabel?: boolean;
  size?: 'xs' | 'sm' | 'md';
  className?: string;
  labelClassName?: string;
}) {
  const meta = machineStatusMeta(status);
  return (
    <StatusDot
      tone={meta.tone}
      label={showLabel ? meta.label : undefined}
      size={size}
      pulse={meta.tone === 'running'}
      className={className}
      labelClassName={labelClassName}
    />
  );
}

const CAPABILITY_ORDER = ['streaming', 'interrupt', 'permission_approval', 'tool_calls'] as const;

export function CapabilityList({
  capabilities,
  className,
}: {
  capabilities: AgentCapabilities;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'flex flex-wrap items-center gap-x-1.5 gap-y-1 font-mono text-[9px] leading-4 text-muted-foreground',
        className,
      )}
    >
      {CAPABILITY_ORDER.map((capability, index) => (
        <Fragment key={capability}>
          {index > 0 ? (
            <span aria-hidden="true" className="text-faint">
              ·
            </span>
          ) : null}
          <span
            title={`${capability} ${capabilities[capability] ? 'supported' : 'not supported'}`}
            className={capabilities[capability] ? undefined : 'text-faint line-through'}
          >
            {capability}
          </span>
        </Fragment>
      ))}
    </div>
  );
}

/** Short relative age used by the ledger heartbeat and updated columns. */
export function formatCompactRelativeTime(value?: string | null): string {
  if (!value) return '—';
  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) return '—';

  let duration = (Date.now() - timestamp) / 1000;
  const divisions = [
    { amount: 60, unit: 's' },
    { amount: 60, unit: 'm' },
    { amount: 24, unit: 'h' },
    { amount: 7, unit: 'd' },
    { amount: 4.34524, unit: 'w' },
    { amount: 12, unit: 'mo' },
    { amount: Number.POSITIVE_INFINITY, unit: 'y' },
  ] as const;

  for (const division of divisions) {
    if (Math.abs(duration) < division.amount) {
      return `${Math.round(duration)}${division.unit}`;
    }
    duration /= division.amount;
  }
  return '—';
}

export function formatCompactRelativeAgo(value?: string | null): string {
  const compact = formatCompactRelativeTime(value);
  if (compact === '—') return compact;
  return compact.startsWith('-') ? `in ${compact.slice(1)}` : `${compact} ago`;
}

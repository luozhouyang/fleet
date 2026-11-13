import type { Machine } from '@agent-fleet/protocol-types';
import { Badge, Button, Card, StatusDot } from '@agent-fleet/ui';
import { Link } from '@tanstack/react-router';
import { MachineStatusBadge } from '@/components/status/StatusBadge';
import { formatRelativeTime } from '@/lib/utils/format';

export function MachineCard({
  machine,
  onDelete,
  onAddWorkspace,
}: {
  machine: Machine;
  onDelete: (machine: Machine) => void;
  onAddWorkspace: (machine: Machine) => void;
}) {
  const providers = machine.providers ?? [];
  const workspaceCount = Math.max(machine.workspace_count ?? 0, machine.workspaces?.length ?? 0);

  return (
    <Card className="group flex flex-col overflow-hidden transition-colors hover:border-slate-700">
      <div className="flex items-start justify-between gap-3 border-b border-slate-800/70 p-4">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h2 className="truncate text-sm font-semibold text-slate-100">{machine.name}</h2>
            <MachineStatusBadge status={machine.status} />
          </div>
          <p className="mt-1 truncate font-mono text-[11px] text-slate-500">
            {machine.hostname}
          </p>
        </div>
        <button
          type="button"
          onClick={() => onDelete(machine)}
          aria-label={`Delete ${machine.name}`}
          title="Delete machine"
          className="flex size-8 shrink-0 items-center justify-center rounded-md text-slate-600 transition-colors hover:bg-red-500/10 hover:text-red-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-400/60"
        >
          <svg viewBox="0 0 20 20" fill="none" className="size-4" aria-hidden="true">
            <path
              d="M4 6h12M8 6V4.5h4V6m-6.5 0 .7 10h7.6l.7-10M8.5 9v4.5M11.5 9v4.5"
              stroke="currentColor"
              strokeWidth="1.4"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>
      </div>

      <div className="grid grid-cols-2 gap-3 p-4 text-xs">
        <div>
          <div className="text-[10px] uppercase tracking-wider text-slate-600">Platform</div>
          <div className="mt-1 font-mono text-slate-300">
            {machine.os} / {machine.arch}
          </div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wider text-slate-600">Daemon</div>
          <div className="mt-1 font-mono text-slate-300">v{machine.daemon_version}</div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wider text-slate-600">Last seen</div>
          <div className="mt-1 text-slate-300">{formatRelativeTime(machine.last_seen_at)}</div>
        </div>
        <div>
          <div className="text-[10px] uppercase tracking-wider text-slate-600">Workspaces</div>
          <div className="mt-1 text-slate-300">{workspaceCount}</div>
        </div>
      </div>

      <div className="flex-1 space-y-3 border-t border-slate-800/70 p-4">
        <div className="flex items-center justify-between">
          <span className="text-[10px] uppercase tracking-wider text-slate-600">Providers</span>
          <span className="text-[11px] text-slate-600">{providers.length}</span>
        </div>
        {providers.length ? (
          <div className="flex flex-wrap gap-1.5">
            {providers.slice(0, 4).map((provider) => (
              <Badge key={provider.id} tone={provider.available ? 'teal' : 'slate'}>
                <StatusDot tone={provider.available ? 'teal' : 'slate'} />
                {provider.display_name}
              </Badge>
            ))}
            {providers.length > 4 ? (
              <Badge tone="slate">+{providers.length - 4}</Badge>
            ) : null}
          </div>
        ) : (
          <p className="text-xs text-slate-600">No provider capabilities reported.</p>
        )}
      </div>

      <div className="flex items-center gap-2 border-t border-slate-800/70 bg-slate-950/20 p-3">
        <Button
          variant="primary"
          size="sm"
          className="flex-1"
          onClick={() => onAddWorkspace(machine)}
        >
          Add workspace
        </Button>
        <Link
          to="/machines/$machineId"
          params={{ machineId: machine.id }}
          className="inline-flex h-8 flex-1 items-center justify-center rounded-md border border-slate-700 bg-slate-900 px-3 text-xs font-medium text-slate-200 transition-colors hover:border-slate-600 hover:bg-slate-800 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60"
        >
          View details
        </Link>
      </div>
    </Card>
  );
}

import type { AgentCapabilities, ProviderInfo } from '@agent-fleet/protocol-types';
import { Badge, Card, StatusDot } from '@agent-fleet/ui';

const capabilityLabels: Array<[keyof AgentCapabilities, string]> = [
  ['streaming', 'Streaming'],
  ['interrupt', 'Interrupt'],
  ['resume', 'Resume'],
  ['permission_approval', 'Permissions'],
  ['structured_events', 'Structured events'],
  ['structured_diff', 'Structured diff'],
  ['session_persistence', 'Persistence'],
  ['tool_calls', 'Tool calls'],
  ['images', 'Images'],
];

export function ProviderCard({ provider }: { provider: ProviderInfo }) {
  return (
    <Card className="p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="truncate text-sm font-semibold text-slate-100">
              {provider.display_name}
            </h3>
            <Badge tone="neutral">{provider.kind}</Badge>
          </div>
          <p className="mt-1 font-mono text-[11px] text-slate-500">
            {provider.id}
            {provider.version ? ` · v${provider.version}` : ''}
          </p>
        </div>
        <Badge tone={provider.available ? 'green' : 'slate'}>
          <StatusDot tone={provider.available ? 'green' : 'slate'} />
          {provider.available ? 'Available' : 'Unavailable'}
        </Badge>
      </div>

      <div className="mt-4 grid grid-cols-2 gap-x-4 gap-y-2 border-t border-slate-800/70 pt-4">
        {capabilityLabels.map(([key, label]) => {
          const supported = provider.capabilities[key];
          return (
            <div
              key={key}
              className={
                supported
                  ? 'flex items-center gap-2 text-xs text-slate-300'
                  : 'flex items-center gap-2 text-xs text-slate-600'
              }
            >
              <span
                className={
                  supported
                    ? 'flex size-4 items-center justify-center rounded-sm bg-teal-500/15 text-teal-300'
                    : 'flex size-4 items-center justify-center rounded-sm bg-slate-800/70 text-slate-600'
                }
                aria-hidden="true"
              >
                {supported ? (
                  <svg viewBox="0 0 16 16" fill="none" className="size-3">
                    <path
                      d="m3.5 8.5 3 3 6-7"
                      stroke="currentColor"
                      strokeWidth="1.8"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                ) : (
                  <span className="h-px w-2 bg-current" />
                )}
              </span>
              {label}
            </div>
          );
        })}
      </div>

      {provider.models?.length ? (
        <div className="mt-4 flex flex-wrap gap-1.5 border-t border-slate-800/70 pt-4">
          {provider.models.map((model) => (
            <Badge key={model} tone="slate" className="font-mono">
              {model}
            </Badge>
          ))}
        </div>
      ) : null}
    </Card>
  );
}

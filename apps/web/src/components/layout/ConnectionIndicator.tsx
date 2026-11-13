import type { EventConnectionState } from '@agent-fleet/api-client';
import { Badge, type Tone } from '@agent-fleet/ui';
import { useEventConnectionState } from '@/lib/events/useEventConnection';

const stateConfig: Record<
  EventConnectionState,
  { label: string; tone: Tone; pulse: boolean }
> = {
  connected: { label: 'Events live', tone: 'green', pulse: true },
  connecting: { label: 'Connecting', tone: 'amber', pulse: true },
  reconnecting: { label: 'Reconnecting', tone: 'amber', pulse: true },
  disconnected: { label: 'Events offline', tone: 'slate', pulse: false },
  failed: { label: 'Events failed', tone: 'red', pulse: false },
};

export function ConnectionIndicator() {
  const state = useEventConnectionState();
  const config = stateConfig[state];

  return (
    <Badge tone={config.tone} title={`Event WebSocket: ${config.label}`}>
      <span className="relative flex size-2" aria-hidden="true">
        {config.pulse ? (
          <span className="absolute inline-flex size-full animate-ping rounded-full bg-current opacity-50" />
        ) : null}
        <span className="relative inline-flex size-2 rounded-full bg-current" />
      </span>
      <span className="hidden sm:inline">{config.label}</span>
    </Badge>
  );
}

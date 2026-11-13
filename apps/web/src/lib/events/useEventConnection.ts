import type { EventConnectionState } from '@agent-fleet/api-client';
import { useSyncExternalStore } from 'react';
import { eventClient } from '@/lib/api/client';

function subscribe(listener: () => void): () => void {
  return eventClient.subscribeState(listener);
}

function getSnapshot(): EventConnectionState {
  return eventClient.getState();
}

export function useEventConnectionState(): EventConnectionState {
  return useSyncExternalStore(subscribe, getSnapshot, () => 'disconnected');
}

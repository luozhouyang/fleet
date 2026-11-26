/**
 * Frame-batched delivery of agent events into the session store.
 *
 * This module owns the browser scheduling (`requestAnimationFrame` plus a
 * timer fallback), which is why it lives apart from `session-store`: the store
 * itself stays framework- and DOM-free so its reducer tests run in the node
 * environment.
 */

import type { AgentEvent } from '@agent-fleet/protocol-types';
import { useSessionStore } from '@/lib/session-store';

let pendingEvents: AgentEvent[] = [];
let frameHandle: number | null = null;
let timerHandle: number | null = null;

const MAX_PENDING_EVENTS = 5_000;
const FALLBACK_FLUSH_MS = 250;

function flushAgentEvents(): void {
  frameHandle = null;
  if (timerHandle !== null) {
    window.clearTimeout(timerHandle);
    timerHandle = null;
  }
  if (!pendingEvents.length) return;
  const batch = pendingEvents;
  pendingEvents = [];
  useSessionStore.getState().ingestAgentEvents(batch);
}

/**
 * Batches all agent events into one store update per animation frame. A timer
 * fallback keeps the queue draining while the tab is backgrounded, where
 * requestAnimationFrame is paused, and a hard cap flushes early if a very
 * verbose session outpaces the frame.
 */
export function queueAgentEvents(events: AgentEvent | AgentEvent[]): void {
  if (Array.isArray(events)) pendingEvents.push(...events);
  else pendingEvents.push(events);

  if (pendingEvents.length >= MAX_PENDING_EVENTS) {
    flushAgentEvents();
    return;
  }

  if (frameHandle === null) {
    frameHandle = window.requestAnimationFrame(flushAgentEvents);
  }
  if (timerHandle === null) {
    timerHandle = window.setTimeout(flushAgentEvents, FALLBACK_FLUSH_MS);
  }
}

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible' && pendingEvents.length) flushAgentEvents();
  });
}

import { useSyncExternalStore } from 'react';

export interface LagNotice {
  /** Number of platform events the gateway reported as skipped. */
  skipped: number;
  /** When the lag event was observed, in epoch milliseconds. */
  at: number;
}

let notice: LagNotice | null = null;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of [...listeners]) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function reportLag(skipped: number): void {
  notice = { skipped: Math.max(0, skipped), at: Date.now() };
  emit();
}

export function dismissLag(): void {
  if (!notice) return;
  notice = null;
  emit();
}

export function getLagNotice(): LagNotice | null {
  return notice;
}

export function useLagNotice(): LagNotice | null {
  return useSyncExternalStore(subscribe, getLagNotice, () => null);
}

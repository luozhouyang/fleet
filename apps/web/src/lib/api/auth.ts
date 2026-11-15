import { useSyncExternalStore } from 'react';

/**
 * Tracks whether the API has rejected the current credentials with a 401.
 *
 * The flag is process-local (not persisted): on reload the app attempts its
 * normal requests and the gate reappears as soon as any of them is rejected.
 */
let unauthorized = false;
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

export function markUnauthorized(): void {
  if (unauthorized) return;
  unauthorized = true;
  emit();
}

export function clearUnauthorized(): void {
  if (!unauthorized) return;
  unauthorized = false;
  emit();
}

export function isUnauthorized(): boolean {
  return unauthorized;
}

export function useUnauthorized(): boolean {
  return useSyncExternalStore(subscribe, isUnauthorized, () => false);
}

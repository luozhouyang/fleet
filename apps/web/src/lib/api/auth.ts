import { useSyncExternalStore } from 'react';

/**
 * Why the API rejected the current credentials.
 *
 * `token_expired` is a distinct state so the auth gate can explain that the
 * token was valid but has passed its expiry instead of asking for a token that
 * may still be correct.
 */
export type UnauthorizedReason = 'unauthorized' | 'token_expired';

/**
 * Tracks whether the API has rejected the current credentials with a 401.
 *
 * The flag is process-local (not persisted): on reload the app attempts its
 * normal requests and the gate reappears as soon as any of them is rejected.
 */
let reason: UnauthorizedReason | null = null;
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

export function markUnauthorized(next: UnauthorizedReason = 'unauthorized'): void {
  // An expired credential is the more precise explanation; later 401s from
  // in-flight requests after the token is cleared must not hide it.
  if (reason === 'token_expired' && next === 'unauthorized') return;
  if (reason === next) return;
  reason = next;
  emit();
}

export function clearUnauthorized(): void {
  if (reason === null) return;
  reason = null;
  emit();
}

export function isUnauthorized(): boolean {
  return reason !== null;
}

export function getUnauthorizedReason(): UnauthorizedReason | null {
  return reason;
}

export function useUnauthorized(): boolean {
  return useSyncExternalStore(subscribe, isUnauthorized, () => false);
}

export function useUnauthorizedReason(): UnauthorizedReason | null {
  return useSyncExternalStore(subscribe, getUnauthorizedReason, () => null);
}

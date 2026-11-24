import { useSyncExternalStore } from 'react';

/**
 * Organization selected by this browser.
 *
 * The value is sent as `X-Fleet-Org` on every REST request and used when
 * minting the event-socket ticket. An empty value means "use the server
 * default" (a team organization when the user has one, else the personal
 * space).
 */
const STORAGE_KEY = 'agent-fleet.active-org';
const CHANGED_EVENT = 'agent-fleet:active-org-changed';

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export function getActiveOrgId(): string {
  const value = safeStorage()?.getItem(STORAGE_KEY);
  return value?.trim() ?? '';
}

export function setActiveOrgId(orgId: string): void {
  const normalized = orgId.trim();
  const storage = safeStorage();
  if (normalized) storage?.setItem(STORAGE_KEY, normalized);
  else storage?.removeItem(STORAGE_KEY);
  window.dispatchEvent(new Event(CHANGED_EVENT));
}

function subscribe(listener: () => void): () => void {
  const onStorage = (event: StorageEvent) => {
    if (event.key === STORAGE_KEY) listener();
  };
  window.addEventListener(CHANGED_EVENT, listener);
  window.addEventListener('storage', onStorage);
  return () => {
    window.removeEventListener(CHANGED_EVENT, listener);
    window.removeEventListener('storage', onStorage);
  };
}

/** The selected organization, or an empty string for the server default. */
export function useActiveOrgId(): string {
  return useSyncExternalStore(subscribe, getActiveOrgId, () => '');
}

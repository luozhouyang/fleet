import { useSyncExternalStore } from 'react';

export const DEFAULT_API_TOKEN = 'dev-token';

const STORAGE_KEY = 'agent-fleet.api-token';
const GIT_WRITE_STORAGE_KEY = 'agent-fleet.git-write-token';
const TOKEN_CHANGED_EVENT = 'agent-fleet:token-changed';

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export function getApiToken(): string {
  const storage = safeStorage();
  const value = storage?.getItem(STORAGE_KEY);
  return value && value.trim() ? value : DEFAULT_API_TOKEN;
}

/**
 * Optional dedicated Git write token. Empty means "not configured"; the API
 * client then omits the `X-Agent-Fleet-Git-Token` header.
 */
export function getGitWriteToken(): string {
  const storage = safeStorage();
  const value = storage?.getItem(GIT_WRITE_STORAGE_KEY);
  return value?.trim() ?? '';
}

/** Persist or clear the optional Git write token. */
export function setGitWriteToken(token: string): void {
  const normalized = token.trim();
  const storage = safeStorage();
  if (normalized) storage?.setItem(GIT_WRITE_STORAGE_KEY, normalized);
  else storage?.removeItem(GIT_WRITE_STORAGE_KEY);
}

export function setApiToken(token: string): void {
  const normalized = token.trim() || DEFAULT_API_TOKEN;
  const storage = safeStorage();
  storage?.setItem(STORAGE_KEY, normalized);
  window.dispatchEvent(new Event(TOKEN_CHANGED_EVENT));
}

function subscribeToken(listener: () => void): () => void {
  const onStorage = (event: StorageEvent) => {
    if (event.key === STORAGE_KEY) listener();
  };
  window.addEventListener(TOKEN_CHANGED_EVENT, listener);
  window.addEventListener('storage', onStorage);
  return () => {
    window.removeEventListener(TOKEN_CHANGED_EVENT, listener);
    window.removeEventListener('storage', onStorage);
  };
}

export function useApiToken(): string {
  return useSyncExternalStore(subscribeToken, getApiToken, () => DEFAULT_API_TOKEN);
}

import { useSyncExternalStore } from 'react';
import { clearUnauthorized } from './auth';

/**
 * The dev build injects `VITE_DEFAULT_API_TOKEN` so a locally generated token
 * can be used without typing it. Production builds leave it empty, which
 * forces the auth gate to ask for a token.
 */
const DEFAULT_API_TOKEN = (import.meta.env.VITE_DEFAULT_API_TOKEN ?? '').trim();

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
 * client then omits the `X-Fleet-Git-Token` header.
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
  const normalized = token.trim();
  const storage = safeStorage();
  if (normalized) storage?.setItem(STORAGE_KEY, normalized);
  else storage?.removeItem(STORAGE_KEY);
  // A new token is a fresh credential attempt; let the app re-enter and let
  // any rejected request surface again if the token is still wrong.
  clearUnauthorized();
  window.dispatchEvent(new Event(TOKEN_CHANGED_EVENT));
}

/**
 * Forget the stored token without clearing the rejection state.
 *
 * Used when the API reports `token_expired`: the credential is known to be
 * unusable, so it must not be retried silently, while the auth gate still
 * needs to explain why it appeared.
 */
export function clearApiToken(): void {
  safeStorage()?.removeItem(STORAGE_KEY);
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

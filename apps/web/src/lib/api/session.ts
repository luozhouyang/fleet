import { ApiClient, ApiClientError } from '@agent-fleet/api-client';
import { API_BASE_URL } from './config';

/** Result of exchanging a bearer token for a browser session. */
export type SessionUpgrade = 'session' | 'legacy_token' | 'unauthorized' | 'unavailable';

/**
 * The double-submit CSRF token.
 *
 * The server exposes the CSRF value in a readable cookie so the page can echo
 * it in `X-CSRF-Token`; the matching session cookie stays HttpOnly.
 */
export function getCsrfToken(): string | null {
  const prefix = 'fleet_csrf=';
  const match = document.cookie.split('; ').find((row) => row.startsWith(prefix));
  return match ? decodeURIComponent(match.slice(prefix.length)) : null;
}

/** A client without app-level callbacks, used before the session exists. */
function sessionClient(token?: string): ApiClient {
  return new ApiClient({
    baseUrl: API_BASE_URL,
    token,
    csrfToken: getCsrfToken,
  });
}

/**
 * Exchange a user bearer token for an HttpOnly session cookie.
 *
 * A `403` means the token is the legacy bootstrap credential, which cannot own
 * a browser session; the caller keeps using it as a bearer token.
 */
export async function upgradeToSession(token: string): Promise<SessionUpgrade> {
  try {
    await sessionClient(token).createBrowserSession();
    return 'session';
  } catch (error) {
    if (error instanceof ApiClientError) {
      if (error.status === 401) return 'unauthorized';
      if (error.status === 403) return 'legacy_token';
    }
    return 'unavailable';
  }
}

/** Whether the browser already holds a live session cookie. */
export async function hasBrowserSession(): Promise<boolean> {
  try {
    await sessionClient().me();
    return true;
  } catch {
    return false;
  }
}

/** Revoke the session cookie server-side and clear it in the browser. */
export async function logoutBrowserSession(): Promise<void> {
  try {
    await sessionClient().logout();
  } catch {
    // Cookie clearing is best effort; the server revokes when it can.
  }
}

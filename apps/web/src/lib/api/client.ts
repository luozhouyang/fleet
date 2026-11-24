import { ApiClient, EventClient } from '@agent-fleet/api-client';
import { markUnauthorized } from './auth';
import { API_BASE_URL, EVENT_URL } from './config';
import { getActiveOrgId, setActiveOrgId } from './organization';
import { getCsrfToken } from './session';
import { clearApiToken, getApiToken, getGitWriteToken } from './token';

export { API_BASE_URL };

export const apiClient = new ApiClient({
  baseUrl: API_BASE_URL,
  token: getApiToken,
  gitWriteToken: getGitWriteToken,
  orgId: getActiveOrgId,
  csrfToken: getCsrfToken,
  onUnauthorized: (error) => {
    if (error.code === 'token_expired') {
      clearApiToken();
      markUnauthorized('token_expired');
    } else {
      markUnauthorized('unauthorized');
    }
  },
  onForbidden: (error) => {
    // A stored organization the user was removed from rejects every request.
    // Drop the selection so the next request falls back to the server's
    // default organization.
    if (getActiveOrgId() && error.message.startsWith('you are not a member of organization')) {
      setActiveOrgId('');
    }
  },
});

export const eventClient = new EventClient({
  url: EVENT_URL,
  // Browsers cannot set an Authorization header on a WebSocket, so every
  // connection mints a ticket bound to the active organization.
  getTicket: async () => {
    const ticket = await apiClient.createRealtimeTicket(getActiveOrgId() || null);
    return ticket.ticket;
  },
});

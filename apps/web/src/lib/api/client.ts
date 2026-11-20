import { ApiClient, EventClient } from '@agent-fleet/api-client';
import { markUnauthorized } from './auth';
import { clearApiToken, getApiToken, getGitWriteToken } from './token';

export const API_BASE_URL = (import.meta.env.VITE_API_BASE_URL ?? '/api/v1').replace(/\/+$/, '');

const eventUrl = API_BASE_URL.startsWith('http') ? `${API_BASE_URL}/events` : '/api/v1/events';

export const apiClient = new ApiClient({
  baseUrl: API_BASE_URL,
  token: getApiToken,
  gitWriteToken: getGitWriteToken,
  onUnauthorized: (error) => {
    if (error.code === 'token_expired') {
      clearApiToken();
      markUnauthorized('token_expired');
    } else {
      markUnauthorized('unauthorized');
    }
  },
});

export const eventClient = new EventClient({
  url: eventUrl,
  getToken: getApiToken,
});

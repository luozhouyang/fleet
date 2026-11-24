/** HTTP API base URL, including the version prefix. */
export const API_BASE_URL = (import.meta.env.VITE_API_BASE_URL ?? '/api/v1').replace(/\/+$/, '');

/** Browser event WebSocket URL. */
export const EVENT_URL = API_BASE_URL.startsWith('http')
  ? `${API_BASE_URL}/events`
  : '/api/v1/events';

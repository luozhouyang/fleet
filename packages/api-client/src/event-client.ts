import type { PlatformEvent } from '@agent-fleet/protocol-types';
import { ApiClientError } from './errors';

export type EventConnectionState =
  'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'failed';

export interface EventClientOptions {
  /** Absolute or relative URL. Defaults to `/api/v1/events`. */
  url?: string;
  /**
   * Mint a single-use ticket for the next connection attempt. Preferred by
   * browsers, which cannot attach an Authorization header to a WebSocket.
   * The provider is called on every (re)connect.
   */
  getTicket?: () => Promise<string | null | undefined>;
  /**
   * Bearer token appended as `?token=`. Kept for non-browser clients; the
   * browser should provide `getTicket` instead.
   */
  getToken?: () => string | null | undefined;
  minReconnectDelayMs?: number;
  maxReconnectDelayMs?: number;
  /**
   * Reconnect when no message arrives for this long. The server emits a
   * `platform.heartbeat` every 15 seconds, so the default tolerates three
   * missed heartbeats. Set to `0` to disable the watchdog.
   */
  heartbeatTimeoutMs?: number;
  webSocketImpl?: typeof WebSocket;
  onError?: (error: unknown) => void;
}

type StoredHandler = (event: PlatformEvent<unknown>) => void;

interface Subscription {
  typeOrPrefix: string;
  handler: StoredHandler;
}

const DEFAULT_URL = '/api/v1/events';
const DEFAULT_MIN_DELAY = 500;
const DEFAULT_MAX_DELAY = 15_000;
const DEFAULT_HEARTBEAT_TIMEOUT = 45_000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isPlatformEvent(value: unknown): value is PlatformEvent {
  return (
    isRecord(value) &&
    typeof value.event_id === 'string' &&
    typeof value.type === 'string' &&
    'payload' in value
  );
}

/**
 * Browser event socket with exponential-backoff reconnect and typed,
 * prefix-based subscriptions.
 *
 * Reconnection is driven by the WebSocket close lifecycle, plus a heartbeat
 * watchdog: a socket that stays silent past `heartbeatTimeoutMs` is dropped and
 * reconnected, because a half-open connection (a suspended laptop, a dropped
 * mobile network) may never deliver a `close` event.
 */
export class EventClient {
  private readonly url: string;
  private readonly getTicket?: () => Promise<string | null | undefined>;
  private readonly getToken?: () => string | null | undefined;
  private readonly minReconnectDelayMs: number;
  private readonly maxReconnectDelayMs: number;
  private readonly heartbeatTimeoutMs: number;
  private readonly WebSocketImpl: typeof WebSocket;
  private readonly onError?: (error: unknown) => void;

  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private watchdogTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempt = 0;
  private shouldReconnect = false;
  private state: EventConnectionState = 'disconnected';
  /** Bumped on disconnect so a pending ticket fetch cannot open a stale socket. */
  private generation = 0;

  private readonly subscriptions = new Set<Subscription>();
  private readonly stateListeners = new Set<() => void>();

  constructor(options: EventClientOptions = {}) {
    this.url = options.url ?? DEFAULT_URL;
    this.getTicket = options.getTicket;
    this.getToken = options.getToken;
    this.minReconnectDelayMs = options.minReconnectDelayMs ?? DEFAULT_MIN_DELAY;
    this.maxReconnectDelayMs = options.maxReconnectDelayMs ?? DEFAULT_MAX_DELAY;
    this.heartbeatTimeoutMs = options.heartbeatTimeoutMs ?? DEFAULT_HEARTBEAT_TIMEOUT;
    this.WebSocketImpl = options.webSocketImpl ?? WebSocket;
    this.onError = options.onError;
  }

  /** Starts the connection. Returns once the first connection attempt is started. */
  connect(): Promise<void> {
    this.shouldReconnect = true;
    if (
      this.socket &&
      (this.socket.readyState === WebSocket.OPEN || this.socket.readyState === WebSocket.CONNECTING)
    ) {
      return Promise.resolve();
    }
    this.clearReconnectTimer();
    void this.openSocket();
    return Promise.resolve();
  }

  disconnect(): void {
    this.shouldReconnect = false;
    this.generation += 1;
    this.clearReconnectTimer();
    this.clearWatchdogTimer();
    this.setState('disconnected');
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.onopen = null;
      socket.onmessage = null;
      socket.onerror = null;
      socket.onclose = null;
      try {
        if (socket.readyState === WebSocket.OPEN) socket.close(1000, 'client disconnect');
        else if (socket.readyState === WebSocket.CONNECTING) socket.close();
      } catch {
        // The socket was already closing; no action needed.
      }
    }
  }

  /** Forces a fresh connection while keeping the reconnect policy enabled. */
  reconnect(): Promise<void> {
    this.disconnect();
    return this.connect();
  }

  getState(): EventConnectionState {
    return this.state;
  }

  subscribeState(listener: () => void): () => void {
    this.stateListeners.add(listener);
    return () => {
      this.stateListeners.delete(listener);
    };
  }

  /**
   * Subscribes to an event type. Prefix matching is supported, e.g. `agent`
   * receives `agent.event` and `agent.session`; `*` receives everything.
   */
  subscribe<T = unknown>(
    typeOrPrefix: string,
    handler: (event: PlatformEvent<T>) => void,
  ): () => void {
    const subscription: Subscription = {
      typeOrPrefix,
      handler: handler as StoredHandler,
    };
    this.subscriptions.add(subscription);
    return () => {
      this.subscriptions.delete(subscription);
    };
  }

  private buildUrl(ticket?: string | null): string {
    const origin = typeof window !== 'undefined' ? window.location.origin : 'http://localhost';
    const url = new URL(this.url, origin);

    if (url.protocol === 'http:') url.protocol = 'ws:';
    if (url.protocol === 'https:') url.protocol = 'wss:';

    if (ticket?.trim()) {
      url.searchParams.set('ticket', ticket.trim());
    } else {
      // Non-browser fallback. The admin server only accepts tickets for
      // browser sockets; this path exists for tests and future clients.
      const token = this.getToken?.();
      if (token?.trim()) url.searchParams.set('token', token.trim());
    }
    return url.toString();
  }

  private isAuthError(error: unknown): boolean {
    return error instanceof ApiClientError && (error.status === 401 || error.status === 403);
  }

  private async openSocket(): Promise<void> {
    this.setState(this.reconnectAttempt > 0 ? 'reconnecting' : 'connecting');
    const generation = this.generation;

    let ticket: string | null = null;
    if (this.getTicket) {
      try {
        ticket = (await this.getTicket()) ?? null;
      } catch (error) {
        if (generation !== this.generation) return;
        this.reportError(error);
        this.setState('failed');
        // A rejected credential cannot be fixed by reconnecting; wait for a
        // new token or organization selection to call `connect` again.
        if (!this.isAuthError(error)) this.scheduleReconnect();
        return;
      }
      if (generation !== this.generation) return;
      if (!ticket) {
        this.reportError(new Error('Event socket ticket is unavailable'));
        this.setState('failed');
        this.scheduleReconnect();
        return;
      }
    }

    let socket: WebSocket;
    try {
      socket = new this.WebSocketImpl(this.buildUrl(ticket));
    } catch (error) {
      this.reportError(error);
      this.setState('failed');
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;

    socket.onopen = () => {
      if (this.socket !== socket) return;
      this.reconnectAttempt = 0;
      this.setState('connected');
      this.armWatchdog(socket);
    };

    socket.onmessage = (message: MessageEvent) => {
      if (this.socket !== socket) return;
      // Any inbound frame proves the connection is alive, not just heartbeats.
      this.armWatchdog(socket);
      if (typeof message.data !== 'string') return;
      let parsed: unknown;
      try {
        parsed = JSON.parse(message.data);
      } catch {
        return;
      }
      if (!isPlatformEvent(parsed)) return;
      this.dispatch(parsed);
    };

    socket.onerror = () => {
      // The close event that follows drives reconnection.
      this.reportError(new Error('Event WebSocket error'));
    };

    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.clearWatchdogTimer();
      this.setState('disconnected');
      if (this.shouldReconnect) this.scheduleReconnect();
    };
  }

  private armWatchdog(socket: WebSocket): void {
    this.clearWatchdogTimer();
    if (this.heartbeatTimeoutMs <= 0) return;
    this.watchdogTimer = setTimeout(() => this.onWatchdogTimeout(socket), this.heartbeatTimeoutMs);
  }

  /**
   * A silent socket may be half-open, in which case `close` never fires. Drop
   * it ourselves and let the reconnect policy open a fresh one.
   */
  private onWatchdogTimeout(socket: WebSocket): void {
    this.watchdogTimer = null;
    if (this.socket !== socket) return;
    this.socket = null;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onerror = null;
    socket.onclose = null;
    try {
      socket.close(4000, 'heartbeat timeout');
    } catch {
      // The socket was already closing; no action needed.
    }
    this.setState('disconnected');
    if (this.shouldReconnect) this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (!this.shouldReconnect || this.reconnectTimer) return;
    this.setState('reconnecting');
    const base = Math.min(
      this.maxReconnectDelayMs,
      this.minReconnectDelayMs * 1.8 ** this.reconnectAttempt,
    );
    this.reconnectAttempt += 1;
    const jitter = 0.8 + Math.random() * 0.4;
    const delay = Math.round(base * jitter);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.openSocket();
    }, delay);
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private clearWatchdogTimer(): void {
    if (this.watchdogTimer) {
      clearTimeout(this.watchdogTimer);
      this.watchdogTimer = null;
    }
  }

  private dispatch(event: PlatformEvent): void {
    for (const subscription of [...this.subscriptions]) {
      if (!this.matches(subscription.typeOrPrefix, event.type)) continue;
      try {
        subscription.handler(event);
      } catch (error) {
        this.reportError(error);
      }
    }
  }

  private matches(typeOrPrefix: string, eventType: string): boolean {
    if (typeOrPrefix === '*') return true;
    if (eventType === typeOrPrefix) return true;
    const prefix = typeOrPrefix.endsWith('.') ? typeOrPrefix : `${typeOrPrefix}.`;
    return eventType.startsWith(prefix);
  }

  private setState(state: EventConnectionState): void {
    if (this.state === state) return;
    this.state = state;
    for (const listener of [...this.stateListeners]) listener();
  }

  private reportError(error: unknown): void {
    this.onError?.(error);
  }
}

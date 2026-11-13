import type { PlatformEvent } from '@agent-fleet/protocol-types';

export type EventConnectionState =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'failed';

export interface EventClientOptions {
  /** Absolute or relative URL. Defaults to `/api/v1/events`. */
  url?: string;
  getToken?: () => string | null | undefined;
  minReconnectDelayMs?: number;
  maxReconnectDelayMs?: number;
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
 * The client deliberately knows nothing about heartbeats. The browser and the
 * platform gateway are responsible for liveness; reconnecting is driven only by
 * the WebSocket close lifecycle.
 */
export class EventClient {
  private readonly url: string;
  private readonly getToken?: () => string | null | undefined;
  private readonly minReconnectDelayMs: number;
  private readonly maxReconnectDelayMs: number;
  private readonly WebSocketImpl: typeof WebSocket;
  private readonly onError?: (error: unknown) => void;

  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempt = 0;
  private shouldReconnect = false;
  private state: EventConnectionState = 'disconnected';

  private readonly subscriptions = new Set<Subscription>();
  private readonly stateListeners = new Set<() => void>();

  constructor(options: EventClientOptions = {}) {
    this.url = options.url ?? DEFAULT_URL;
    this.getToken = options.getToken;
    this.minReconnectDelayMs = options.minReconnectDelayMs ?? DEFAULT_MIN_DELAY;
    this.maxReconnectDelayMs = options.maxReconnectDelayMs ?? DEFAULT_MAX_DELAY;
    this.WebSocketImpl = options.webSocketImpl ?? WebSocket;
    this.onError = options.onError;
  }

  /** Starts the connection. Returns once the first connection attempt is started. */
  connect(): Promise<void> {
    this.shouldReconnect = true;
    if (
      this.socket &&
      (this.socket.readyState === WebSocket.OPEN ||
        this.socket.readyState === WebSocket.CONNECTING)
    ) {
      return Promise.resolve();
    }
    this.clearReconnectTimer();
    this.openSocket();
    return Promise.resolve();
  }

  disconnect(): void {
    this.shouldReconnect = false;
    this.clearReconnectTimer();
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

  private buildUrl(): string {
    const origin = typeof window !== 'undefined' ? window.location.origin : 'http://localhost';
    const url = new URL(this.url, origin);

    if (url.protocol === 'http:') url.protocol = 'ws:';
    if (url.protocol === 'https:') url.protocol = 'wss:';

    const token = this.getToken?.();
    if (token?.trim()) url.searchParams.set('token', token.trim());
    return url.toString();
  }

  private openSocket(): void {
    this.setState(this.reconnectAttempt > 0 ? 'reconnecting' : 'connecting');
    let socket: WebSocket;
    try {
      socket = new this.WebSocketImpl(this.buildUrl());
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
    };

    socket.onmessage = (message: MessageEvent) => {
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
      this.setState('disconnected');
      if (this.shouldReconnect) this.scheduleReconnect();
    };
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
      this.openSocket();
    }, delay);
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
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

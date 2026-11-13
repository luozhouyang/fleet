import {
  PermissionStatus,
  type AgentEvent,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentSession,
  type SessionDetail,
} from '@agent-fleet/protocol-types';
import { create } from 'zustand';
import {
  createSessionView,
  hydrateSessionView,
  reduceAgentEvents,
  type SessionViewState,
} from './session-reducer';

interface SessionStore {
  sessions: Record<string, SessionViewState>;
  ingestAgentEvents: (events: AgentEvent[]) => void;
  hydrateSession: (detail: SessionDetail) => void;
  upsertSession: (session: AgentSession) => void;
  addMessage: (message: AgentMessage) => void;
  removeMessage: (sessionId: string, messageId: string) => void;
  setPermissionStatus: (
    sessionId: string,
    permissionId: string,
    status: PermissionStatus,
  ) => void;
  mergePermissions: (permissions: AgentPermissionRequest[]) => void;
}

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: {},

  ingestAgentEvents: (events) => {
    set((state) => {
      const bySession = new Map<string, AgentEvent[]>();
      for (const event of events) {
        const group = bySession.get(event.session_id);
        if (group) group.push(event);
        else bySession.set(event.session_id, [event]);
      }

      const sessions = { ...state.sessions };
      for (const [sessionId, sessionEvents] of bySession) {
        const current = sessions[sessionId] ?? createSessionView();
        sessions[sessionId] = reduceAgentEvents(current, sessionEvents);
      }
      return { sessions };
    });
  },

  hydrateSession: (detail) => {
    set((state) => {
      const current = state.sessions[detail.session.id] ?? createSessionView();
      return {
        sessions: {
          ...state.sessions,
          [detail.session.id]: hydrateSessionView(current, detail),
        },
      };
    });
  },

  upsertSession: (session) => {
    set((state) => {
      const current = state.sessions[session.id] ?? createSessionView();
      return {
        sessions: {
          ...state.sessions,
          [session.id]: {
            ...current,
            session,
            statusOverride: undefined,
            lastError: session.last_error ?? current.lastError,
          },
        },
      };
    });
  },

  addMessage: (message) => {
    set((state) => {
      const current = state.sessions[message.session_id] ?? createSessionView();
      const exists = Boolean(current.messages[message.id]);
      return {
        sessions: {
          ...state.sessions,
          [message.session_id]: {
            ...current,
            messages: { ...current.messages, [message.id]: message },
            messageOrder: exists
              ? current.messageOrder
              : [...current.messageOrder, message.id],
          },
        },
      };
    });
  },

  removeMessage: (sessionId, messageId) => {
    set((state) => {
      const current = state.sessions[sessionId];
      if (!current) return state;
      const messages = { ...current.messages };
      delete messages[messageId];
      return {
        sessions: {
          ...state.sessions,
          [sessionId]: {
            ...current,
            messages,
            messageOrder: current.messageOrder.filter((id) => id !== messageId),
          },
        },
      };
    });
  },

  setPermissionStatus: (sessionId, permissionId, status) => {
    set((state) => {
      const current = state.sessions[sessionId];
      const permission = current?.permissions[permissionId];
      if (!current || !permission) return state;
      return {
        sessions: {
          ...state.sessions,
          [sessionId]: {
            ...current,
            permissions: {
              ...current.permissions,
              [permissionId]: {
                ...permission,
                status,
                resolved_at: new Date().toISOString(),
              },
            },
          },
        },
      };
    });
  },

  mergePermissions: (permissions) => {
    if (!permissions.length) return;
    set((state) => {
      const bySession = new Map<string, AgentPermissionRequest[]>();
      for (const permission of permissions) {
        const group = bySession.get(permission.session_id);
        if (group) group.push(permission);
        else bySession.set(permission.session_id, [permission]);
      }

      const sessions = { ...state.sessions };
      for (const [sessionId, incoming] of bySession) {
        let current = sessions[sessionId] ?? createSessionView();
        for (const permission of incoming) {
          const existing = current.permissions[permission.id];
          if (
            existing &&
            existing.status !== PermissionStatus.Pending &&
            permission.status === PermissionStatus.Pending
          ) {
            continue;
          }
          current = {
            ...current,
            permissions: { ...current.permissions, [permission.id]: permission },
            permissionOrder: current.permissionOrder.includes(permission.id)
              ? current.permissionOrder
              : [...current.permissionOrder, permission.id],
          };
        }
        sessions[sessionId] = current;
      }
      return { sessions };
    });
  },
}));

let pendingEvents: AgentEvent[] = [];
let frameHandle: number | null = null;

function flushAgentEvents(): void {
  frameHandle = null;
  if (!pendingEvents.length) return;
  const batch = pendingEvents;
  pendingEvents = [];
  useSessionStore.getState().ingestAgentEvents(batch);
}

/**
 * Batches all agent events into one store update per animation frame. Deltas
 * therefore update React at most once per frame instead of once per event.
 */
export function queueAgentEvents(events: AgentEvent | AgentEvent[]): void {
  if (Array.isArray(events)) pendingEvents.push(...events);
  else pendingEvents.push(events);

  if (frameHandle === null) {
    frameHandle = window.requestAnimationFrame(flushAgentEvents);
  }
}

export function selectSessionView(
  state: { sessions: Record<string, SessionViewState> },
  sessionId: string,
): SessionViewState | undefined {
  return state.sessions[sessionId];
}

import {
  PermissionStatus,
  QuestionStatus,
  type AgentEvent,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentQuestionAnswer,
  type AgentQuestionRequest,
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
  removeSession: (sessionId: string) => void;
  hydrateSession: (detail: SessionDetail) => void;
  upsertSession: (session: AgentSession) => void;
  addMessage: (message: AgentMessage) => void;
  removeMessage: (sessionId: string, messageId: string) => void;
  setPermissionStatus: (sessionId: string, permissionId: string, status: PermissionStatus) => void;
  mergePermissions: (permissions: AgentPermissionRequest[]) => void;
  setQuestionStatus: (sessionId: string, questionId: string, status: QuestionStatus) => void;
  setQuestionAnswers: (
    sessionId: string,
    questionId: string,
    answers: AgentQuestionAnswer[],
  ) => void;
  mergeQuestions: (questions: AgentQuestionRequest[]) => void;
}

/**
 * Sessions whose deletion has been observed.
 *
 * Events for a session can still sit in the batching queue when the deletion
 * arrives. Ingesting them would recreate a view from scratch, leaving a ghost
 * in memory that no later event can remove. A `session.created` for the same
 * id is the one thing that legitimately brings the id back.
 */
const deletedSessions = new Set<string>();

export const useSessionStore = create<SessionStore>((set) => ({
  sessions: {},

  ingestAgentEvents: (events) => {
    set((state) => {
      const bySession = new Map<string, AgentEvent[]>();
      for (const event of events) {
        if (deletedSessions.has(event.session_id)) {
          if (event.type !== 'session.created') continue;
          deletedSessions.delete(event.session_id);
        }
        const group = bySession.get(event.session_id);
        if (group) group.push(event);
        else bySession.set(event.session_id, [event]);
      }
      if (bySession.size === 0) return state;

      const sessions = { ...state.sessions };
      for (const [sessionId, sessionEvents] of bySession) {
        const current = sessions[sessionId] ?? createSessionView();
        sessions[sessionId] = reduceAgentEvents(current, sessionEvents);
      }
      return { sessions };
    });
  },

  removeSession: (sessionId) => {
    deletedSessions.add(sessionId);
    set((state) => {
      if (!state.sessions[sessionId]) return state;
      const sessions = { ...state.sessions };
      delete sessions[sessionId];
      return { sessions };
    });
  },

  hydrateSession: (detail) => {
    // A fetched detail means the session exists; clear any stale deletion.
    deletedSessions.delete(detail.session.id);
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
    deletedSessions.delete(session.id);
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
            messageOrder: exists ? current.messageOrder : [...current.messageOrder, message.id],
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

  setQuestionStatus: (sessionId, questionId, status) => {
    set((state) => {
      const current = state.sessions[sessionId];
      const question = current?.questions[questionId];
      if (!current || !question) return state;
      if (question.status !== QuestionStatus.Pending && status === QuestionStatus.Pending) {
        return state;
      }
      return {
        sessions: {
          ...state.sessions,
          [sessionId]: {
            ...current,
            questions: {
              ...current.questions,
              [questionId]: {
                ...question,
                status,
                resolved_at: new Date().toISOString(),
              },
            },
          },
        },
      };
    });
  },

  setQuestionAnswers: (sessionId, questionId, answers) => {
    set((state) => {
      const current = state.sessions[sessionId];
      if (!current) return state;
      return {
        sessions: {
          ...state.sessions,
          [sessionId]: {
            ...current,
            questionAnswers: { ...current.questionAnswers, [questionId]: answers },
          },
        },
      };
    });
  },

  mergeQuestions: (questions) => {
    if (!questions.length) return;
    set((state) => {
      const bySession = new Map<string, AgentQuestionRequest[]>();
      for (const question of questions) {
        const group = bySession.get(question.session_id);
        if (group) group.push(question);
        else bySession.set(question.session_id, [question]);
      }

      const sessions = { ...state.sessions };
      for (const [sessionId, incoming] of bySession) {
        let current = sessions[sessionId] ?? createSessionView();
        for (const question of incoming) {
          const existing = current.questions[question.id];
          if (
            existing &&
            existing.status !== QuestionStatus.Pending &&
            question.status === QuestionStatus.Pending
          ) {
            continue;
          }
          current = {
            ...current,
            questions: { ...current.questions, [question.id]: question },
            questionOrder: current.questionOrder.includes(question.id)
              ? current.questionOrder
              : [...current.questionOrder, question.id],
          };
        }
        sessions[sessionId] = current;
      }
      return { sessions };
    });
  },
}));

export function selectSessionView(
  state: { sessions: Record<string, SessionViewState> },
  sessionId: string,
): SessionViewState | undefined {
  return state.sessions[sessionId];
}

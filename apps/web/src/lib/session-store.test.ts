import { MessageRole, SessionStatus, type AgentEvent } from '@agent-fleet/protocol-types';
import { beforeEach, describe, expect, it } from 'vitest';
import { useSessionStore } from './session-store';

function event(type: string, payload: unknown = {}, extra: Partial<AgentEvent> = {}): AgentEvent {
  return {
    event_id: `evt_${type}`,
    session_id: 's_1',
    sequence: 1,
    type: type as AgentEvent['type'],
    timestamp: '2026-01-01T10:00:00Z',
    provider: 'acp',
    payload: payload as never,
    ...extra,
  };
}

function assistantMessage(id: string, content: string) {
  return {
    message: {
      id,
      session_id: 's_1',
      role: MessageRole.Assistant,
      content,
      created_at: '2026-01-01T10:00:00Z',
    },
  };
}

beforeEach(() => {
  useSessionStore.setState({ sessions: {} });
});

describe('deleted sessions', () => {
  it('ignores events that were batched before the deletion', () => {
    const store = useSessionStore.getState();
    store.ingestAgentEvents([event('turn.started', {}, { turn_id: 'turn_1' })]);
    store.removeSession('s_1');

    // A delta queued before the delete still arrives after it in the batch.
    store.ingestAgentEvents([event('message.completed', assistantMessage('m_1', 'late'))]);
    expect(useSessionStore.getState().sessions.s_1).toBeUndefined();
  });

  it('brings the session back on a fresh create', () => {
    const store = useSessionStore.getState();
    store.removeSession('s_1');
    store.ingestAgentEvents([
      event('session.created', {
        session: {
          id: 's_1',
          machine_id: 'm_1',
          provider: 'acp',
          agent_type: 'acp',
          status: SessionStatus.Running,
          cwd: '/repo',
          created_at: '2026-01-01T10:00:00Z',
          updated_at: '2026-01-01T10:00:00Z',
        },
      }),
    ]);
    expect(useSessionStore.getState().sessions.s_1?.session?.id).toBe('s_1');
  });

  it('drops the guard once a hydrated detail proves the session exists', () => {
    const store = useSessionStore.getState();
    store.removeSession('s_1');
    store.hydrateSession({
      session: {
        id: 's_1',
        machine_id: 'm_1',
        provider: 'acp',
        agent_type: 'acp',
        status: SessionStatus.Completed,
        cwd: '/repo',
        created_at: '2026-01-01T10:00:00Z',
        updated_at: '2026-01-01T10:00:00Z',
      },
      turns: [],
      messages: [],
      tool_calls: [],
      permissions: [],
      questions: [],
    });
    store.ingestAgentEvents([event('message.completed', assistantMessage('m_1', 'hello'))]);
    const view = useSessionStore.getState().sessions.s_1;
    expect(view?.messages.m_1.content).toBe('hello');
  });
});

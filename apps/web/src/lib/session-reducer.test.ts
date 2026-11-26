import {
  MessageRole,
  SessionStatus,
  ToolCallStatus,
  TurnStatus,
  type AgentEvent,
  type AgentEventType,
  type AgentPlanEntry,
  type SessionDetail,
} from '@agent-fleet/protocol-types';
import { describe, expect, it } from 'vitest';
import {
  NOTICE_LIMIT,
  TERMINAL_BUFFER_LIMIT,
  createSessionView,
  hydrateSessionView,
  reduceAgentEvents,
  type SessionViewState,
} from './session-reducer';

let sequence = 0;

function event(type: string, payload: unknown, extra: Partial<AgentEvent> = {}): AgentEvent {
  sequence += 1;
  return {
    event_id: `evt_${sequence}`,
    session_id: 's_1',
    sequence,
    type: type as AgentEventType,
    timestamp: `2026-01-01T10:00:${String(sequence % 60).padStart(2, '0')}Z`,
    provider: 'acp',
    payload: payload as never,
    ...extra,
  };
}

function reduce(
  events: AgentEvent[],
  view: SessionViewState = createSessionView(),
): SessionViewState {
  return reduceAgentEvents(view, events);
}

function planEntry(content: string, status: AgentPlanEntry['status']): AgentPlanEntry {
  return { content, status, priority: 'unknown' };
}

function detail(overrides: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: {
      id: 's_1',
      machine_id: 'm_1',
      provider: 'acp',
      agent_type: 'acp',
      status: SessionStatus.Completed,
      cwd: '/repo',
      created_at: '2026-01-01T10:00:00Z',
      updated_at: '2026-01-01T11:00:00Z',
    },
    turns: [],
    messages: [],
    tool_calls: [],
    permissions: [],
    questions: [],
    ...overrides,
  };
}

describe('plan, notice and telemetry events', () => {
  it('keeps only the latest plan', () => {
    const first = event('agent.plan.updated', {
      plan: {
        id: 'plan_1',
        session_id: 's_1',
        entries: [{ content: 'a', status: 'pending' }],
        updated_at: '2026-01-01T10:00:00Z',
      },
    });
    const second = event('agent.plan.updated', {
      plan: {
        id: 'plan_1',
        session_id: 's_1',
        entries: [{ content: 'a', status: 'completed' }],
        updated_at: '2026-01-01T10:01:00Z',
      },
    });
    const view = reduce([first, second]);
    expect(view.plan?.entries[0].status).toBe('completed');
  });

  it('caps notices at the retention limit', () => {
    const events = Array.from({ length: NOTICE_LIMIT + 5 }, (_, index) =>
      event('agent.notice', { kind: 'retry', level: 'info', message: `notice ${index}` }),
    );
    const view = reduce(events);
    expect(view.notices).toHaveLength(NOTICE_LIMIT);
    expect(view.notices.at(-1)?.message).toBe(`notice ${NOTICE_LIMIT + 4}`);
  });

  it('keeps the last usage and activity values', () => {
    const view = reduce([
      event('agent.usage.updated', { used: 10, size: 100 }),
      event('agent.usage.updated', { used: 20, size: 100 }),
      event('agent.activity.changed', { state: 'running', agent_initiated: false }),
      event('agent.activity.changed', { state: 'idle', agent_initiated: false }),
    ]);
    expect(view.usage).toMatchObject({ used: 20 });
    expect(view.activity).toMatchObject({ state: 'idle' });
  });

  it('appends terminal output and keeps only the tail', () => {
    const chunk = 'x'.repeat(TERMINAL_BUFFER_LIMIT);
    const view = reduce([
      event('agent.terminal.output', { terminal_id: 'term_1', data: '', text: chunk, done: false }),
      event('agent.terminal.output', {
        terminal_id: 'term_1',
        data: '',
        text: 'TAIL',
        done: true,
        command: 'cargo test',
      }),
    ]);
    const terminal = view.terminals.term_1;
    expect(terminal.done).toBe(true);
    expect(terminal.command).toBe('cargo test');
    expect(terminal.text.endsWith('TAIL')).toBe(true);
    expect(terminal.text.length).toBeLessThanOrEqual(TERMINAL_BUFFER_LIMIT);
  });

  it('ignores raw provider events instead of storing them', () => {
    const view = reduce([event('agent.raw_event', { kind: 'whatever' })]);
    expect(view.notices).toHaveLength(0);
    expect(view.plan).toBeNull();
  });

  it('drops the session on deletion', () => {
    const view = reduce(
      [
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
        event('session.deleted', {}),
      ],
      createSessionView(),
    );
    expect(view.session).toBeNull();
    expect(view.statusOverride).toBe(SessionStatus.Terminated);
  });
});

describe('streaming message tracking', () => {
  it('marks an assistant message as streaming until it completes', () => {
    const view = reduce([
      event('turn.started', {}, { turn_id: 'turn_1' }),
      event('message.started', { message_id: 'm_1', role: MessageRole.Assistant }),
    ]);
    expect(view.streamingMessageId).toBe('m_1');

    const completed = reduce(
      [
        event('message.completed', {
          message: {
            id: 'm_1',
            session_id: 's_1',
            turn_id: 'turn_1',
            role: MessageRole.Assistant,
            content: 'done',
            created_at: '2026-01-01T10:00:05Z',
          },
        }),
      ],
      view,
    );
    expect(completed.streamingMessageId).toBeNull();
  });

  it('clears the flag when a turn ends without a final message', () => {
    const view = reduce([
      event('message.started', { message_id: 'm_1', role: MessageRole.Assistant }),
      event('turn.failed', {}, { turn_id: 'turn_1' }),
    ]);
    expect(view.streamingMessageId).toBeNull();
  });

  it('does not flag a user message as streaming', () => {
    const view = reduce([event('message.started', { message_id: 'u_1', role: MessageRole.User })]);
    expect(view.streamingMessageId).toBeNull();
  });
});

describe('hydration', () => {
  it('restores the persisted plan and refuses a stale one', () => {
    const base: SessionViewState = {
      ...createSessionView(),
      plan: {
        id: 'plan_1',
        session_id: 's_1',
        entries: [{ content: 'newer', status: 'completed', priority: 'unknown' }],
        updated_at: '2026-01-01T11:00:00Z',
      },
    };
    const stale = detail({
      plan: {
        id: 'plan_1',
        session_id: 's_1',
        entries: [planEntry('older', 'pending')],
        updated_at: '2026-01-01T10:00:00Z',
      },
    });
    const view = hydrateSessionView(base, stale);
    expect(view.plan?.entries[0].content).toBe('newer');
  });

  it('adopts a persisted plan when the live view has none', () => {
    const stored = detail({
      plan: {
        id: 'plan_1',
        session_id: 's_1',
        entries: [planEntry('stored', 'pending')],
        updated_at: '2026-01-01T10:00:00Z',
      },
    });
    const view = hydrateSessionView(createSessionView(), stored);
    expect(view.plan?.entries[0].content).toBe('stored');
  });

  it('never rolls a terminal turn or tool back to running', () => {
    const base = reduce([
      event('turn.started', {}, { turn_id: 'turn_1' }),
      event('turn.completed', {}, { turn_id: 'turn_1' }),
      event('message.started', { message_id: 'm_1', role: MessageRole.Assistant }),
    ]);
    const snapshot = detail({
      session: {
        ...detail().session,
        status: SessionStatus.Running,
      },
      turns: [
        {
          id: 'turn_1',
          session_id: 's_1',
          status: TurnStatus.Running,
          started_at: '2026-01-01T10:00:00Z',
        },
      ],
      tool_calls: [
        {
          id: 'tool_1',
          session_id: 's_1',
          name: 'Bash',
          status: ToolCallStatus.Running,
          input: {},
          started_at: '2026-01-01T10:00:00Z',
        },
      ],
    });
    const view = hydrateSessionView(base, snapshot);
    expect(view.turns.turn_1.status).toBe(TurnStatus.Completed);
    expect(view.tools.tool_1.status).toBe(ToolCallStatus.Running);
  });
});

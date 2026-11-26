import {
  MessageRole,
  PermissionStatus,
  ToolCallStatus,
  TurnStatus,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentToolCall,
  type AgentTurn,
} from '@agent-fleet/protocol-types';
import { describe, expect, it } from 'vitest';
import type { SessionViewState } from '../session-reducer';
import { createSessionView } from '../session-reducer';
import {
  flattenTranscript,
  projectTranscript,
  transcriptIsLive,
  transcriptRowCount,
} from './transcript-model';

function message(id: string, role: MessageRole, at: string, turnId?: string): AgentMessage {
  return {
    id,
    session_id: 's_1',
    turn_id: turnId,
    role,
    content: id,
    created_at: at,
  };
}

function tool(id: string, at: string, turnId?: string): AgentToolCall {
  return {
    id,
    session_id: 's_1',
    turn_id: turnId,
    name: 'bash',
    status: ToolCallStatus.Completed,
    input: { command: 'ls' },
    output: 'a.txt',
    started_at: at,
  };
}

function permission(id: string, at: string, turnId?: string): AgentPermissionRequest {
  return {
    id,
    session_id: 's_1',
    turn_id: turnId,
    tool_name: 'bash',
    description: 'run ls',
    input: {},
    options: [],
    status: PermissionStatus.Approved,
    created_at: at,
  };
}

function turn(id: string, status: TurnStatus, started: string, completed?: string): AgentTurn {
  return {
    id,
    session_id: 's_1',
    status,
    started_at: started,
    completed_at: completed,
  };
}

function view(): SessionViewState {
  return createSessionView();
}

function addMessage(state: SessionViewState, value: AgentMessage) {
  state.messages[value.id] = value;
  state.messageOrder.push(value.id);
}

function addTool(state: SessionViewState, value: AgentToolCall) {
  state.tools[value.id] = value;
  state.toolOrder.push(value.id);
}

describe('transcript projection', () => {
  it('groups rows by turn and closes each with its status row', () => {
    const state = view();
    state.turns.t_1 = turn(
      't_1',
      TurnStatus.Completed,
      '2026-01-01T10:00:00Z',
      '2026-01-01T10:01:00Z',
    );
    state.turnOrder.push('t_1');
    addMessage(state, message('u1', MessageRole.User, '2026-01-01T10:00:00Z', 't_1'));
    addTool(state, tool('tool1', '2026-01-01T10:00:30Z', 't_1'));
    addMessage(state, message('a1', MessageRole.Assistant, '2026-01-01T10:00:50Z', 't_1'));

    const turns = projectTranscript(state);
    expect(turns).toHaveLength(1);
    expect(turns[0].rows.map((row) => row.kind)).toEqual([
      'message',
      'tool',
      'message',
      'turn-status',
    ]);
    expect(transcriptIsLive(turns)).toBe(false);
  });

  it('opens a turn on a user message even without turn records', () => {
    const state = view();
    addMessage(state, message('u1', MessageRole.User, '2026-01-01T10:00:00Z'));
    addMessage(state, message('a1', MessageRole.Assistant, '2026-01-01T10:00:05Z'));
    addMessage(state, message('u2', MessageRole.User, '2026-01-01T10:01:00Z'));
    addMessage(state, message('a2', MessageRole.Assistant, '2026-01-01T10:01:05Z'));

    const turns = projectTranscript(state);
    expect(turns).toHaveLength(2);
    expect(turns[0].rows.map((row) => row.kind)).toEqual(['message', 'message']);
    expect(turns[1].rows.map((row) => row.kind)).toEqual(['message', 'message']);
  });

  it('interleaves permissions, notices and the plan by time', () => {
    const state = view();
    addMessage(state, message('u1', MessageRole.User, '2026-01-01T10:00:00Z'));
    state.permissions.p1 = permission('p1', '2026-01-01T10:00:10Z');
    state.permissionOrder.push('p1');
    state.notices.push({
      id: 'n1',
      at: '2026-01-01T10:00:20Z',
      kind: 'retry',
      level: 'info',
      message: 'retrying',
    });
    state.plan = {
      id: 'plan_1',
      session_id: 's_1',
      entries: [],
      updated_at: '2026-01-01T10:00:30Z',
    };

    const rows = flattenTranscript(projectTranscript(state));
    expect(rows.map((row) => row.kind)).toEqual(['message', 'permission', 'notice', 'plan']);
  });

  it('marks a running turn as live', () => {
    const state = view();
    state.turns.t_1 = turn('t_1', TurnStatus.Running, '2026-01-01T10:00:00Z');
    addMessage(state, message('u1', MessageRole.User, '2026-01-01T10:00:00Z', 't_1'));
    const turns = projectTranscript(state);
    expect(transcriptIsLive(turns)).toBe(true);
    expect(transcriptRowCount(turns)).toBe(2);
  });

  it('attaches a leading row with no turn to a synthetic turn', () => {
    const state = view();
    state.notices.push({
      id: 'n1',
      at: '2026-01-01T09:59:00Z',
      kind: 'start',
      level: 'info',
      message: 'started',
    });
    const turns = projectTranscript(state);
    expect(turns).toHaveLength(1);
    expect(turns[0].turn).toBeNull();
    expect(turns[0].rows[0].kind).toBe('notice');
  });

  it('keeps transcript order stable for equal timestamps', () => {
    const state = view();
    addMessage(state, message('u1', MessageRole.User, '2026-01-01T10:00:00Z'));
    addTool(state, tool('a-tool', '2026-01-01T10:00:00Z'));
    const keys = flattenTranscript(projectTranscript(state)).map((row) => row.key);
    expect(keys).toEqual(['message:u1', 'tool:a-tool']);
  });
});

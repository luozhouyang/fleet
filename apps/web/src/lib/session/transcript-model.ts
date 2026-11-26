/**
 * Pure transcript projection.
 *
 * The daemon's durable transcript is flat (messages, tool calls, permissions,
 * questions, turns); the UI needs conversation rows grouped by turn, with
 * notices and the plan slotted in temporal order. This module owns that
 * projection so it can be unit tested without React and memoized on the
 * reducer's order arrays.
 */

import {
  MessageRole,
  PermissionStatus,
  QuestionStatus,
  ToolCallStatus,
  TurnStatus,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentQuestionAnswer,
  type AgentQuestionRequest,
  type AgentPlan,
  type AgentToolCall,
  type AgentTurn,
} from '@agent-fleet/protocol-types';
import type { SessionNotice, SessionToolCall } from '../session-reducer';

/** One renderable row in the transcript. */
export type TranscriptRow =
  | { kind: 'message'; key: string; at: string; message: AgentMessage }
  | { kind: 'tool'; key: string; at: string; tool: SessionToolCall }
  | {
      kind: 'permission';
      key: string;
      at: string;
      permission: AgentPermissionRequest;
    }
  | {
      kind: 'question';
      key: string;
      at: string;
      question: AgentQuestionRequest;
      answers?: AgentQuestionAnswer[];
    }
  | { kind: 'notice'; key: string; at: string; notice: SessionNotice }
  | { kind: 'plan'; key: string; at: string; plan: AgentPlan }
  | { kind: 'turn-status'; key: string; at: string; turn: AgentTurn | null };

/** A turn with its rows, in display order. */
export interface TranscriptTurn {
  key: string;
  turn: AgentTurn | null;
  rows: TranscriptRow[];
}

/** Everything the projection needs, structural so tests can build it cheaply. */
export interface TranscriptSource {
  messages: Record<string, AgentMessage>;
  messageOrder: string[];
  tools: Record<string, SessionToolCall>;
  toolOrder: string[];
  permissions: Record<string, AgentPermissionRequest>;
  permissionOrder: string[];
  questions: Record<string, AgentQuestionRequest>;
  questionOrder: string[];
  questionAnswers: Record<string, AgentQuestionAnswer[]>;
  notices: SessionNotice[];
  plan: AgentPlan | null;
  turns: Record<string, AgentTurn>;
  turnOrder: string[];
}

function timestamp(value: string): number {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

/**
 * Tool calls whose output is still streaming and will arrive separately.
 * A completed tool is the only point at which the daemon has final output,
 * so the UI keeps the card visible from the start and fills it in.
 */
export function toolIsLive(tool: AgentToolCall): boolean {
  return tool.status === ToolCallStatus.Pending || tool.status === ToolCallStatus.Running;
}

/** Whether a permission/question still needs a decision. */
export function promptIsPending(prompt: AgentPermissionRequest | AgentQuestionRequest): boolean {
  return prompt.status === PermissionStatus.Pending || prompt.status === QuestionStatus.Pending;
}

/**
 * Group rows into turns.
 *
 * The daemon supplies `turn_id` on most rows; anything without one is attached
 * to the nearest preceding user message (or a leading synthetic turn), which is
 * what the provider protocol guarantees anyway: a user prompt opens a turn.
 */
export function projectTranscript(source: TranscriptSource): TranscriptTurn[] {
  const rows: TranscriptRow[] = [];

  for (const id of source.messageOrder) {
    const message = source.messages[id];
    if (!message) continue;
    rows.push({ kind: 'message', key: `message:${id}`, at: message.created_at, message });
  }
  for (const id of source.toolOrder) {
    const tool = source.tools[id];
    if (!tool) continue;
    rows.push({ kind: 'tool', key: `tool:${id}`, at: tool.started_at, tool });
  }
  for (const id of source.permissionOrder) {
    const permission = source.permissions[id];
    if (!permission) continue;
    rows.push({
      kind: 'permission',
      key: `permission:${id}`,
      at: permission.created_at,
      permission,
    });
  }
  for (const id of source.questionOrder) {
    const question = source.questions[id];
    if (!question) continue;
    rows.push({
      kind: 'question',
      key: `question:${id}`,
      at: question.created_at,
      question,
      answers: source.questionAnswers[id],
    });
  }
  for (const notice of source.notices) {
    rows.push({ kind: 'notice', key: `notice:${notice.id}`, at: notice.at, notice });
  }
  if (source.plan) {
    rows.push({
      kind: 'plan',
      key: `plan:${source.plan.id}`,
      at: source.plan.updated_at,
      plan: source.plan,
    });
  }

  // Stable sort by time, then by row kind/id so equal timestamps keep a
  // deterministic, readable order (messages before their tools).
  rows.sort((left, right) => {
    const delta = timestamp(left.at) - timestamp(right.at);
    return delta !== 0 ? delta : left.key.localeCompare(right.key);
  });

  const turns: TranscriptTurn[] = [];
  const byKey = new Map<string, TranscriptTurn>();
  let openTurn: TranscriptTurn | null = null;

  const turnFor = (key: string, turn: AgentTurn | null): TranscriptTurn => {
    const existing = byKey.get(key);
    if (existing) {
      if (turn && !existing.turn) existing.turn = turn;
      return existing;
    }
    const created: TranscriptTurn = { key, turn, rows: [] };
    byKey.set(key, created);
    turns.push(created);
    return created;
  };

  for (const row of rows) {
    const turnId = turnIdOf(row);
    // A user message always starts a turn, even if a late tool from the
    // previous turn shares its timestamp.
    if (row.kind === 'message' && row.message.role === MessageRole.User) {
      openTurn = turnFor(turnId ?? row.message.id, turnId ? (source.turns[turnId] ?? null) : null);
      openTurn.rows.push(row);
      continue;
    }
    if (turnId) {
      openTurn = turnFor(turnId, source.turns[turnId] ?? null);
    } else if (!openTurn) {
      openTurn = turnFor('turn:start', null);
    }
    openTurn.rows.push(row);
  }

  // The turn-status row always closes its turn, whatever the timestamps say:
  // it summarizes the work above it rather than participating in the order.
  for (const turn of turns) {
    if (!turn.turn) continue;
    turn.rows.push({
      kind: 'turn-status',
      key: `turn-status:${turn.turn.id}`,
      at: turn.turn.completed_at ?? turn.turn.started_at,
      turn: turn.turn,
    });
  }

  return turns.filter((turn) => turn.rows.length > 0);
}

function turnIdOf(row: TranscriptRow): string | undefined {
  switch (row.kind) {
    case 'message':
      return row.message.turn_id;
    case 'tool':
      return row.tool.turn_id;
    case 'permission':
      return row.permission.turn_id;
    case 'question':
      return row.question.turn_id;
    default:
      return undefined;
  }
}

/** Flatten turns into the rows the virtualizer renders. */
export function flattenTranscript(turns: readonly TranscriptTurn[]): TranscriptRow[] {
  const rows: TranscriptRow[] = [];
  for (const turn of turns) {
    for (const row of turn.rows) rows.push(row);
  }
  return rows;
}

/** Total row count, for the virtualization threshold. */
export function transcriptRowCount(turns: readonly TranscriptTurn[]): number {
  let count = 0;
  for (const turn of turns) count += turn.rows.length;
  return count;
}

/**
 * Whether the latest turn still has work in flight.
 *
 * Drives the "streaming" caret and the composer's Stop button.
 */
export function transcriptIsLive(turns: readonly TranscriptTurn[]): boolean {
  const last = turns.at(-1)?.turn;
  if (!last) return false;
  return last.status === TurnStatus.Running;
}

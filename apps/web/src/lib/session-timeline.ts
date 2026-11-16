import type {
  AgentMessage,
  AgentPermissionRequest,
  AgentQuestionAnswer,
  AgentQuestionRequest,
} from '@agent-fleet/protocol-types';
import type { SessionToolCall, SessionViewState } from './session-reducer';

export type TimelineEntry =
  | { kind: 'message'; id: string; at: string; message: AgentMessage }
  | { kind: 'tool'; id: string; at: string; tool: SessionToolCall }
  | { kind: 'permission'; id: string; at: string; permission: AgentPermissionRequest }
  | {
      kind: 'question';
      id: string;
      at: string;
      question: AgentQuestionRequest;
      answers?: AgentQuestionAnswer[];
    };

function timestamp(value: string): number {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

export function buildTimeline(view: SessionViewState): TimelineEntry[] {
  const entries: TimelineEntry[] = [];

  for (const id of view.messageOrder) {
    const message = view.messages[id];
    if (message) entries.push({ kind: 'message', id, at: message.created_at, message });
  }
  for (const id of view.toolOrder) {
    const tool = view.tools[id];
    if (tool) entries.push({ kind: 'tool', id, at: tool.started_at, tool });
  }
  for (const id of view.permissionOrder) {
    const permission = view.permissions[id];
    if (permission) {
      entries.push({
        kind: 'permission',
        id,
        at: permission.created_at,
        permission,
      });
    }
  }
  for (const id of view.questionOrder) {
    const question = view.questions[id];
    if (question) {
      entries.push({
        kind: 'question',
        id,
        at: question.created_at,
        question,
        answers: view.questionAnswers[id],
      });
    }
  }

  return entries.sort((left, right) => {
    const delta = timestamp(left.at) - timestamp(right.at);
    return delta !== 0 ? delta : left.id.localeCompare(right.id);
  });
}

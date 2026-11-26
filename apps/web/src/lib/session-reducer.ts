import {
  MessageRole,
  PermissionStatus,
  QuestionStatus,
  SessionStatus,
  ToolCallStatus,
  TurnStatus,
  type AgentEvent,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentQuestionAnswer,
  type AgentQuestionRequest,
  type AgentSession,
  type AgentToolCall,
  type AgentTurn,
  type ApiError,
  type ActivityChangedPayload,
  type AgentPlan,
  type MessageDeltaPayload,
  type MessageStartedPayload,
  type NoticePayload,
  type PermissionEventPayload,
  type PlanUpdatedPayload,
  type QuestionEventPayload,
  type SessionDetail,
  type StatusChangedPayload,
  type TerminalOutputPayload,
  type ToolDeltaPayload,
  type ToolEventPayload,
  type ToolStatusPayload,
  type UsageUpdatedPayload,
} from '@agent-fleet/protocol-types';

/** A durable agent notice, kept in order for the transcript. */
export interface SessionNotice {
  id: string;
  at: string;
  kind: string;
  level: string;
  message: string;
}

/**
 * Selector over a session's open agent terminals.
 *
 * Kept as a helper so components share one stable shape, and so the count can
 * be derived without rebuilding the whole record.
 */
export function terminalCount(view: SessionViewState | undefined): number {
  return view ? Object.keys(view.terminals).length : 0;
}

/** Retained bytes of agent terminal output per terminal id. */
export const TERMINAL_BUFFER_LIMIT = 64 * 1024;
/** Maximum retained notices per session. */
export const NOTICE_LIMIT = 200;

export interface SessionToolCall extends AgentToolCall {
  /** Raw streaming JSON while `tool.input.delta` events arrive. */
  inputDraft?: string;
}

export interface SessionViewState {
  session: AgentSession | null;
  statusOverride?: SessionStatus;
  lastError?: ApiError;
  messages: Record<string, AgentMessage>;
  messageOrder: string[];
  tools: Record<string, SessionToolCall>;
  toolOrder: string[];
  permissions: Record<string, AgentPermissionRequest>;
  permissionOrder: string[];
  questions: Record<string, AgentQuestionRequest>;
  questionOrder: string[];
  /** Answers submitted from this browser; the wire model does not echo them. */
  questionAnswers: Record<string, AgentQuestionAnswer[]>;
  turns: Record<string, AgentTurn>;
  turnOrder: string[];
  /** Latest agent plan; plans replace each other rather than accumulating. */
  plan: AgentPlan | null;
  /** Agent notices in arrival order, capped at {@link NOTICE_LIMIT}. */
  notices: SessionNotice[];
  /** Latest usage report from the agent. */
  usage: UsageUpdatedPayload | null;
  /** Latest foreground activity. */
  activity: ActivityChangedPayload | null;
  /** Agent-owned terminal output, display-only, capped per terminal. */
  terminals: Record<string, { text: string; command?: string; cwd?: string; done: boolean }>;
  /**
   * The assistant message currently being written, tracked explicitly rather
   * than inferred from the session status: a session can stay `running` while
   * a finished message waits in the transcript.
   */
  streamingMessageId: string | null;
  seenEventIds: string[];
  hydrated: boolean;
}

export function createSessionView(session: AgentSession | null = null): SessionViewState {
  return {
    session,
    statusOverride: undefined,
    lastError: session?.last_error,
    messages: {},
    messageOrder: [],
    tools: {},
    toolOrder: [],
    permissions: {},
    permissionOrder: [],
    questions: {},
    questionOrder: [],
    questionAnswers: {},
    turns: {},
    turnOrder: [],
    plan: null,
    notices: [],
    usage: null,
    activity: null,
    terminals: {},
    streamingMessageId: null,
    seenEventIds: [],
    hydrated: false,
  };
}

export function effectiveSessionStatus(view: SessionViewState): SessionStatus | undefined {
  return view.statusOverride ?? view.session?.status;
}

/**
 * Whether a session status cannot change without a new session.
 *
 * `Interrupted` is deliberately excluded: a machine can reconnect without a
 * daemon restart and the daemon reports the same session as running again.
 */
function isFinalSessionStatus(status: SessionStatus): boolean {
  return (
    status === SessionStatus.Completed ||
    status === SessionStatus.Failed ||
    status === SessionStatus.Terminated
  );
}

function isTerminalPermission(status: PermissionStatus): boolean {
  return status !== PermissionStatus.Pending;
}

function isTerminalQuestion(status: QuestionStatus): boolean {
  return status !== QuestionStatus.Pending;
}

function isTerminalTurn(status: TurnStatus): boolean {
  return (
    status === TurnStatus.Completed ||
    status === TurnStatus.Interrupted ||
    status === TurnStatus.Failed
  );
}

function isTerminalTool(status: ToolCallStatus): boolean {
  return (
    status === ToolCallStatus.Completed ||
    status === ToolCallStatus.Failed ||
    status === ToolCallStatus.Cancelled
  );
}

function appendUnique(order: string[], ids: string[]): string[] {
  const seen = new Set(order);
  const next = [...order];
  for (const id of ids) {
    if (!seen.has(id)) {
      seen.add(id);
      next.push(id);
    }
  }
  return next;
}

function withStatus(view: SessionViewState, status: SessionStatus): SessionViewState {
  return {
    ...view,
    statusOverride: status,
    session: view.session ? { ...view.session, status } : null,
  };
}

function addMessage(view: SessionViewState, message: AgentMessage): SessionViewState {
  const exists = Boolean(view.messages[message.id]);
  return {
    ...view,
    messages: { ...view.messages, [message.id]: message },
    messageOrder: exists ? view.messageOrder : [...view.messageOrder, message.id],
  };
}

function addTool(view: SessionViewState, tool: SessionToolCall): SessionViewState {
  const exists = Boolean(view.tools[tool.id]);
  return {
    ...view,
    tools: { ...view.tools, [tool.id]: tool },
    toolOrder: exists ? view.toolOrder : [...view.toolOrder, tool.id],
  };
}

function addPermission(
  view: SessionViewState,
  permission: AgentPermissionRequest,
): SessionViewState {
  const exists = Boolean(view.permissions[permission.id]);
  return {
    ...view,
    permissions: { ...view.permissions, [permission.id]: permission },
    permissionOrder: exists ? view.permissionOrder : [...view.permissionOrder, permission.id],
  };
}

function addTurn(view: SessionViewState, turn: AgentTurn): SessionViewState {
  const exists = Boolean(view.turns[turn.id]);
  return {
    ...view,
    turns: { ...view.turns, [turn.id]: turn },
    turnOrder: exists ? view.turnOrder : [...view.turnOrder, turn.id],
  };
}

function reduceEvent(view: SessionViewState, event: AgentEvent): SessionViewState {
  if (view.seenEventIds.includes(event.event_id)) return view;

  let next: SessionViewState = {
    ...view,
    seenEventIds: [...view.seenEventIds.slice(-799), event.event_id],
  };

  switch (event.type) {
    case 'session.created': {
      const snapshot = (event.payload as { session?: AgentSession } | null)?.session;
      if (!snapshot) {
        next = withStatus(next, SessionStatus.Creating);
        break;
      }
      // A snapshot (fresh creation or daemon restart) carries the authoritative
      // session row. Keep locally accumulated history but never roll a terminal
      // view back to a non-terminal status.
      const current = effectiveSessionStatus(next);
      const status =
        current && isFinalSessionStatus(current) && !isFinalSessionStatus(snapshot.status)
          ? current
          : snapshot.status;
      next = {
        ...next,
        session: next.session ? { ...next.session, ...snapshot, status } : { ...snapshot, status },
        statusOverride: status,
        lastError: snapshot.last_error ?? next.lastError,
      };
      break;
    }
    case 'session.started':
    case 'session.resumed':
      next = withStatus(next, SessionStatus.Running);
      break;
    case 'session.terminated':
      next = { ...withStatus(next, SessionStatus.Terminated), streamingMessageId: null };
      break;
    case 'session.error': {
      const error = (event.payload as { error?: ApiError } | null)?.error;
      next = {
        ...next,
        lastError: error,
        session: next.session && error ? { ...next.session, last_error: error } : next.session,
      };
      break;
    }
    case 'agent.status.changed': {
      const payload = event.payload as StatusChangedPayload;
      if (!payload?.status) break;
      // A late or replayed status must not move a final session back to a
      // running one. An interrupted session can be resumed or recovered by
      // the same daemon after a reconnect, so it is not final.
      const current = effectiveSessionStatus(next);
      if (current && isFinalSessionStatus(current) && !isFinalSessionStatus(payload.status)) break;
      next = withStatus(next, payload.status);
      break;
    }
    case 'turn.started': {
      if (!event.turn_id) break;
      const current = next.turns[event.turn_id];
      // A turn that already reached a terminal state must not be reopened by a
      // replayed `turn.started`.
      if (current && isTerminalTurn(current.status)) break;
      next = addTurn(next, {
        id: event.turn_id,
        session_id: event.session_id,
        status: TurnStatus.Running,
        started_at: current?.started_at ?? event.timestamp,
      });
      break;
    }
    case 'turn.completed':
    case 'turn.interrupted':
    case 'turn.failed': {
      if (!event.turn_id) break;
      // A turn ending without a final message (interrupt, failure) must not
      // leave the last row looking like it is still streaming.
      next = { ...next, streamingMessageId: null };
      const current = next.turns[event.turn_id] ?? {
        id: event.turn_id,
        session_id: event.session_id,
        status: TurnStatus.Running,
        started_at: event.timestamp,
      };
      const status =
        event.type === 'turn.completed'
          ? TurnStatus.Completed
          : event.type === 'turn.interrupted'
            ? TurnStatus.Interrupted
            : TurnStatus.Failed;
      next = addTurn(next, {
        ...current,
        status,
        completed_at: event.timestamp,
      });
      break;
    }
    case 'message.started': {
      const payload = event.payload as MessageStartedPayload;
      const messageId = payload?.message_id ?? event.message_id;
      if (!messageId) break;
      const existing = next.messages[messageId];
      const role = payload?.role ?? existing?.role ?? MessageRole.Assistant;
      next = addMessage(next, {
        id: messageId,
        session_id: event.session_id,
        turn_id: event.turn_id,
        role,
        content: existing?.content ?? '',
        created_at: existing?.created_at ?? event.timestamp,
      });
      next = {
        ...next,
        streamingMessageId: role === MessageRole.Assistant ? messageId : next.streamingMessageId,
      };
      break;
    }
    case 'message.delta': {
      const payload = event.payload as MessageDeltaPayload;
      const messageId = payload?.message_id ?? event.message_id;
      if (!messageId) break;
      const existing = next.messages[messageId];
      next = addMessage(next, {
        id: messageId,
        session_id: event.session_id,
        turn_id: event.turn_id ?? existing?.turn_id,
        role: existing?.role ?? MessageRole.Assistant,
        content: `${existing?.content ?? ''}${payload?.text ?? ''}`,
        created_at: existing?.created_at ?? event.timestamp,
      });
      break;
    }
    case 'message.completed': {
      const message = (event.payload as { message?: AgentMessage } | null)?.message;
      if (message) {
        // The daemon now confirms user messages. Replace the optimistic local
        // copy with the server id instead of rendering both.
        if (message.role === MessageRole.User) {
          const optimisticId = next.messageOrder.find(
            (id) =>
              id.startsWith('local_') &&
              next.messages[id]?.role === MessageRole.User &&
              next.messages[id]?.content === message.content,
          );
          if (optimisticId) {
            const messages = { ...next.messages };
            delete messages[optimisticId];
            next = {
              ...next,
              messages,
              messageOrder: next.messageOrder.filter((id) => id !== optimisticId),
            };
          }
        }
        next = addMessage(next, message);
        if (next.streamingMessageId === message.id) {
          next = { ...next, streamingMessageId: null };
        }
      }
      break;
    }
    case 'tool.started': {
      const tool = (event.payload as ToolEventPayload)?.tool;
      if (tool) next = addTool(next, { ...tool, inputDraft: undefined });
      break;
    }
    case 'tool.input.delta': {
      const payload = event.payload as ToolDeltaPayload;
      if (!payload?.tool_call_id) break;
      const existing = next.tools[payload.tool_call_id];
      const tool: SessionToolCall = existing
        ? {
            ...existing,
            inputDraft: `${existing.inputDraft ?? ''}${payload.partial_input ?? ''}`,
          }
        : {
            id: payload.tool_call_id,
            session_id: event.session_id,
            turn_id: event.turn_id,
            name: payload.name,
            status: ToolCallStatus.Running,
            input: null,
            inputDraft: payload.partial_input ?? '',
            started_at: event.timestamp,
          };
      next = addTool(next, tool);
      break;
    }
    case 'tool.completed': {
      const tool = (event.payload as ToolEventPayload)?.tool;
      if (tool) next = addTool(next, { ...tool, inputDraft: undefined });
      break;
    }
    case 'tool.failed': {
      const payload = event.payload as ToolStatusPayload;
      if (!payload?.tool_call_id) break;
      const existing = next.tools[payload.tool_call_id];
      if (existing) {
        next = addTool(next, {
          ...existing,
          status: payload.status ?? ToolCallStatus.Failed,
          error: payload.error,
          completed_at: event.timestamp,
        });
      }
      break;
    }
    case 'permission.requested':
    case 'permission.approved':
    case 'permission.rejected': {
      const permission = (event.payload as PermissionEventPayload)?.permission;
      if (permission) next = addPermission(next, permission);
      break;
    }
    case 'question.requested':
    case 'question.answered':
    case 'question.rejected': {
      const question = (event.payload as QuestionEventPayload)?.question;
      if (question) {
        const existing = next.questions[question.id];
        const nextQuestion =
          existing && isTerminalQuestion(existing.status) && !isTerminalQuestion(question.status)
            ? existing
            : question;
        next = {
          ...next,
          questions: { ...next.questions, [question.id]: nextQuestion },
          questionOrder: next.questionOrder.includes(question.id)
            ? next.questionOrder
            : [...next.questionOrder, question.id],
        };
      }
      break;
    }
    case 'agent.plan.updated': {
      const plan = (event.payload as PlanUpdatedPayload)?.plan;
      if (plan) next = { ...next, plan };
      break;
    }
    case 'agent.notice': {
      const payload = event.payload as NoticePayload;
      if (!payload?.message) break;
      const notice: SessionNotice = {
        id: event.event_id,
        at: event.timestamp,
        kind: payload.kind,
        level: payload.level,
        message: payload.message,
      };
      next = {
        ...next,
        notices: [...next.notices, notice].slice(-NOTICE_LIMIT),
      };
      break;
    }
    // Live-only telemetry: forward into the view for display, but keep the
    // last value only. Nothing here is replayed, so a refetch is the recovery
    // path for anything missed.
    case 'agent.usage.updated': {
      next = { ...next, usage: event.payload as UsageUpdatedPayload };
      break;
    }
    case 'agent.activity.changed': {
      next = { ...next, activity: event.payload as ActivityChangedPayload };
      break;
    }
    case 'agent.terminal.output': {
      const payload = event.payload as TerminalOutputPayload;
      if (!payload?.terminal_id) break;
      const existing = next.terminals[payload.terminal_id];
      const text = `${existing?.text ?? ''}${payload.text ?? ''}`;
      // Keep the tail: the newest output is what a reader needs, and the cap
      // bounds memory for a chatty command.
      const retained =
        text.length > TERMINAL_BUFFER_LIMIT ? text.slice(-TERMINAL_BUFFER_LIMIT) : text;
      next = {
        ...next,
        terminals: {
          ...next.terminals,
          [payload.terminal_id]: {
            text: retained,
            command: payload.command ?? existing?.command,
            cwd: payload.cwd ?? existing?.cwd,
            done: payload.done,
          },
        },
      };
      break;
    }
    case 'agent.raw_event':
      break;
    case 'session.deleted':
      // The session row is gone. The caller drops the whole view; keeping an
      // empty one would leave a ghost in memory until reload.
      next = {
        ...next,
        session: null,
        statusOverride: SessionStatus.Terminated,
        streamingMessageId: null,
      };
      break;
    default:
      break;
  }

  return next;
}

export function reduceAgentEvents(view: SessionViewState, events: AgentEvent[]): SessionViewState {
  return events.reduce(reduceEvent, view);
}

export function hydrateSessionView(
  view: SessionViewState,
  detail: SessionDetail,
): SessionViewState {
  const messages = { ...view.messages };
  const serverUserContent = new Set(
    detail.messages
      .filter((message) => message.role === MessageRole.User)
      .map((message) => message.content),
  );

  for (const message of detail.messages) {
    const existing = messages[message.id];
    messages[message.id] =
      existing && !message.content && existing.content
        ? { ...message, content: existing.content }
        : message;
  }

  let messageOrder = [...view.messageOrder];
  for (const message of detail.messages) {
    if (messageOrder.includes(message.id)) continue;
    if (
      message.id.startsWith('local_') &&
      message.role === MessageRole.User &&
      serverUserContent.has(message.content)
    ) {
      continue;
    }
    messageOrder.push(message.id);
  }

  // Drop optimistic user messages once the server has the same content.
  const localToDrop = new Set(
    view.messageOrder.filter((id) => {
      if (!id.startsWith('local_')) return false;
      const message = view.messages[id];
      return message?.role === MessageRole.User && serverUserContent.has(message.content);
    }),
  );
  if (localToDrop.size) {
    messageOrder = messageOrder.filter((id) => !localToDrop.has(id));
    for (const id of localToDrop) delete messages[id];
  }

  const permissions = { ...view.permissions };
  for (const permission of detail.permissions) {
    const existing = permissions[permission.id];
    if (
      existing &&
      isTerminalPermission(existing.status) &&
      !isTerminalPermission(permission.status)
    ) {
      continue;
    }
    permissions[permission.id] = permission;
  }

  const questions = { ...view.questions };
  const detailQuestions = detail.questions ?? [];
  for (const question of detailQuestions) {
    const existing = questions[question.id];
    if (existing && isTerminalQuestion(existing.status) && !isTerminalQuestion(question.status)) {
      continue;
    }
    questions[question.id] = question;
  }

  const tools = { ...view.tools };
  for (const tool of detail.tool_calls ?? []) {
    const existing = tools[tool.id];
    if (existing && isTerminalTool(existing.status) && !isTerminalTool(tool.status)) {
      continue;
    }
    tools[tool.id] = { ...tool, inputDraft: undefined };
  }

  const turns = { ...view.turns };
  for (const turn of detail.turns) {
    const existing = turns[turn.id];
    if (existing && isTerminalTurn(existing.status) && !isTerminalTurn(turn.status)) {
      continue;
    }
    turns[turn.id] = turn;
  }

  // The daemon persists only the latest plan. Keep the live plan when it is
  // newer, so a refetch racing a streamed update cannot roll it back.
  const storedPlan = detail.plan ?? null;
  const plan =
    storedPlan &&
    (!view.plan ||
      (Date.parse(storedPlan.updated_at) || 0) >= (Date.parse(view.plan.updated_at) || 0))
      ? storedPlan
      : view.plan;

  // A refetch is authoritative when the live stream missed a final event
  // (for example during a WebSocket gap), but an in-flight snapshot must never
  // roll a final status back to a running one.
  // A failed session can be resumed back to a running one. The daemon clears
  // `last_error` on a successful resume, which distinguishes a fresh resumed
  // snapshot from a stale refetch racing a live Failed event.
  const failedWasResumed =
    view.statusOverride === SessionStatus.Failed &&
    !isFinalSessionStatus(detail.session.status) &&
    !detail.session.last_error;
  const statusOverride =
    view.statusOverride &&
    isFinalSessionStatus(view.statusOverride) &&
    !isFinalSessionStatus(detail.session.status) &&
    !failedWasResumed
      ? view.statusOverride
      : detail.session.status;

  return {
    ...view,
    session: detail.session,
    statusOverride,
    lastError: detail.session.last_error ?? view.lastError,
    messages,
    messageOrder,
    permissions,
    permissionOrder: appendUnique(
      view.permissionOrder,
      detail.permissions.map((permission) => permission.id),
    ),
    questions,
    questionOrder: appendUnique(
      view.questionOrder,
      detailQuestions.map((question) => question.id),
    ),
    tools,
    toolOrder: appendUnique(
      view.toolOrder,
      (detail.tool_calls ?? []).map((tool) => tool.id),
    ),
    turns,
    turnOrder: appendUnique(
      view.turnOrder,
      detail.turns.map((turn) => turn.id),
    ),
    plan,
    hydrated: true,
  };
}

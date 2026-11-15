import {
  MessageRole,
  PermissionStatus,
  SessionStatus,
  ToolCallStatus,
  TurnStatus,
  type AgentEvent,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentSession,
  type AgentToolCall,
  type AgentTurn,
  type ApiError,
  type MessageDeltaPayload,
  type MessageStartedPayload,
  type PermissionEventPayload,
  type SessionDetail,
  type StatusChangedPayload,
  type ToolDeltaPayload,
  type ToolEventPayload,
  type ToolStatusPayload,
} from '@agent-fleet/protocol-types';

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
  turns: Record<string, AgentTurn>;
  turnOrder: string[];
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
    turns: {},
    turnOrder: [],
    seenEventIds: [],
    hydrated: false,
  };
}

export function effectiveSessionStatus(view: SessionViewState): SessionStatus | undefined {
  return view.statusOverride ?? view.session?.status;
}

function isTerminalSession(status: SessionStatus): boolean {
  return (
    status === SessionStatus.Interrupted ||
    status === SessionStatus.Completed ||
    status === SessionStatus.Failed ||
    status === SessionStatus.Terminated
  );
}

function isTerminalPermission(status: PermissionStatus): boolean {
  return status !== PermissionStatus.Pending;
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
    case 'session.created':
      next = withStatus(next, SessionStatus.Creating);
      break;
    case 'session.started':
    case 'session.resumed':
      next = withStatus(next, SessionStatus.Running);
      break;
    case 'session.terminated':
      next = withStatus(next, SessionStatus.Terminated);
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
      if (payload?.status) next = withStatus(next, payload.status);
      break;
    }
    case 'turn.started': {
      if (!event.turn_id) break;
      const current = next.turns[event.turn_id];
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
      next = addMessage(next, {
        id: messageId,
        session_id: event.session_id,
        turn_id: event.turn_id,
        role: payload?.role ?? existing?.role ?? MessageRole.Assistant,
        content: existing?.content ?? '',
        created_at: existing?.created_at ?? event.timestamp,
      });
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
    case 'agent.raw_event':
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

  // A refetch is authoritative when the live stream missed a terminal event
  // (for example during a WebSocket gap), but an in-flight snapshot must never
  // roll a terminal status back to a running one.
  const statusOverride =
    view.statusOverride &&
    isTerminalSession(view.statusOverride) &&
    !isTerminalSession(detail.session.status)
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
    hydrated: true,
  };
}

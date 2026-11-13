import type {
  ApiError,
  EventId,
  MachineId,
  MessageId,
  SessionId,
  Timestamp,
  TurnId,
  WorkspaceId,
} from './common';

export enum ProviderKind {
  Claude = 'claude',
  Codex = 'codex',
  OpenCode = 'opencode',
  Generic = 'generic',
  Mock = 'mock',
}

export interface AgentCapabilities {
  streaming: boolean;
  interrupt: boolean;
  resume: boolean;
  permission_approval: boolean;
  structured_events: boolean;
  structured_diff: boolean;
  session_persistence: boolean;
  tool_calls: boolean;
  images: boolean;
}

export interface ProviderInfo {
  id: string;
  kind: ProviderKind;
  display_name: string;
  version?: string;
  available: boolean;
  capabilities: AgentCapabilities;
  /** Omitted by the server when empty. */
  models?: string[];
}

export enum SessionStatus {
  Creating = 'creating',
  Starting = 'starting',
  Running = 'running',
  WaitingForInput = 'waiting_for_input',
  WaitingForPermission = 'waiting_for_permission',
  Interrupted = 'interrupted',
  Completed = 'completed',
  Failed = 'failed',
  Terminated = 'terminated',
}

export interface AgentSession {
  id: SessionId;
  machine_id: MachineId;
  workspace_id?: WorkspaceId;
  /** Provider adapter id, e.g. `claude`, `codex`, `mock`. */
  provider: string;
  /** Concrete agent type, e.g. `claude-code`. */
  agent_type: string;
  status: SessionStatus;
  cwd: string;
  title?: string;
  created_at: Timestamp;
  updated_at: Timestamp;
  last_error?: ApiError;
  metadata?: unknown;
}

export interface AgentStartConfig {
  workspace_id?: WorkspaceId;
  cwd: string;
  provider?: string;
  model?: string;
  title?: string;
  system_prompt?: string;
  allowed_tools?: string[];
  env?: Record<string, string>;
  extra?: unknown;
}

export enum TurnStatus {
  Pending = 'pending',
  Running = 'running',
  Completed = 'completed',
  Interrupted = 'interrupted',
  Failed = 'failed',
}

export interface AgentTurn {
  id: TurnId;
  session_id: SessionId;
  status: TurnStatus;
  started_at: Timestamp;
  completed_at?: Timestamp;
  error?: ApiError;
}

export enum MessageRole {
  User = 'user',
  Assistant = 'assistant',
  System = 'system',
}

export interface AgentMessage {
  id: MessageId;
  session_id: SessionId;
  turn_id?: TurnId;
  role: MessageRole;
  content: string;
  created_at: Timestamp;
}

export interface Attachment {
  name: string;
  mime_type?: string;
  data_base64: string;
}

export interface SendMessageRequest {
  content: string;
  attachments?: Attachment[];
}

export enum ToolCallStatus {
  Pending = 'pending',
  Running = 'running',
  Completed = 'completed',
  Failed = 'failed',
  Cancelled = 'cancelled',
}

export interface AgentToolCall {
  id: string;
  session_id: SessionId;
  turn_id?: TurnId;
  name: string;
  status: ToolCallStatus;
  input: unknown;
  output?: unknown;
  error?: string;
  started_at: Timestamp;
  completed_at?: Timestamp;
}

export type ToolCall = AgentToolCall;

export enum PermissionStatus {
  Pending = 'pending',
  Approved = 'approved',
  Rejected = 'rejected',
  TimedOut = 'timed_out',
  Cancelled = 'cancelled',
  DeliveryFailed = 'delivery_failed',
}

export interface AgentPermissionRequest {
  id: string;
  session_id: SessionId;
  turn_id?: TurnId;
  tool_call_id?: string;
  tool_name: string;
  description: string;
  input: unknown;
  status: PermissionStatus;
  created_at: Timestamp;
  resolved_at?: Timestamp;
  expires_at?: Timestamp;
}

export interface PermissionDecision {
  permission_id: string;
  approve: boolean;
  message?: string;
}

export interface RespondPermissionRequest {
  permission_id: string;
  approve: boolean;
  message?: string;
}

export const AGENT_EVENT_TYPES = [
  'session.created',
  'session.started',
  'session.resumed',
  'session.terminated',
  'session.error',
  'turn.started',
  'turn.completed',
  'turn.interrupted',
  'turn.failed',
  'message.started',
  'message.delta',
  'message.completed',
  'tool.started',
  'tool.input.delta',
  'tool.completed',
  'tool.failed',
  'permission.requested',
  'permission.approved',
  'permission.rejected',
  'agent.status.changed',
  'agent.raw_event',
] as const;

export type AgentEventType = (typeof AGENT_EVENT_TYPES)[number];

/** A normalized event produced by a provider adapter. */
export interface AgentEvent<P = unknown> {
  event_id: EventId;
  session_id: SessionId;
  turn_id?: TurnId;
  message_id?: MessageId;
  tool_call_id?: string;
  /** Per-session monotonically increasing sequence number. */
  sequence: number;
  type: AgentEventType;
  timestamp: Timestamp;
  provider: string;
  payload: P;
  raw_event?: unknown;
}

export interface MessageDeltaPayload {
  message_id: MessageId;
  text: string;
  index?: number;
}

export interface MessageStartedPayload {
  message_id: MessageId;
  role: MessageRole;
}

export interface MessageCompletedPayload {
  message: AgentMessage;
}

export interface ToolEventPayload {
  tool: AgentToolCall;
}

export interface ToolDeltaPayload {
  tool_call_id: string;
  name: string;
  partial_input: string;
}

export interface PermissionEventPayload {
  permission: AgentPermissionRequest;
}

export interface StatusChangedPayload {
  status: SessionStatus;
  reason?: string;
}

export interface SessionErrorPayload {
  error: ApiError;
}

export interface ToolStatusPayload {
  tool_call_id: string;
  status: ToolCallStatus;
  error?: string;
}

export interface AgentEventPayloadMap {
  'session.created': unknown;
  'session.started': unknown;
  'session.resumed': unknown;
  'session.terminated': unknown;
  'session.error': SessionErrorPayload;
  'turn.started': unknown;
  'turn.completed': unknown;
  'turn.interrupted': unknown;
  'turn.failed': unknown;
  'message.started': MessageStartedPayload;
  'message.delta': MessageDeltaPayload;
  'message.completed': MessageCompletedPayload;
  'tool.started': ToolEventPayload;
  'tool.input.delta': ToolDeltaPayload;
  'tool.completed': ToolEventPayload;
  'tool.failed': ToolStatusPayload;
  'permission.requested': PermissionEventPayload;
  'permission.approved': PermissionEventPayload;
  'permission.rejected': PermissionEventPayload;
  'agent.status.changed': StatusChangedPayload;
  'agent.raw_event': unknown;
}

export type TypedAgentEvent<T extends AgentEventType> = AgentEvent<AgentEventPayloadMap[T]>;

/** Response shape used by the session detail endpoint / RPC. */
export interface SessionDetail {
  session: AgentSession;
  turns: AgentTurn[];
  messages: AgentMessage[];
  permissions: AgentPermissionRequest[];
}

/** Browser request body for `POST /sessions`. */
export interface CreateAgentSessionRequest {
  provider?: string;
  workspace_id?: WorkspaceId;
  cwd?: string;
  model?: string;
  title?: string;
  system_prompt?: string;
  allowed_tools?: string[];
  env?: Record<string, string>;
  extra?: unknown;
}

/** `CreateAgentSessionRequest` plus the machine routing field added by the control plane. */
export interface CreateSessionRequest extends CreateAgentSessionRequest {
  machine_id: MachineId;
}

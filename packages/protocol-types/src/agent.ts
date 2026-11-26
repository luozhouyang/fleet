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
  Acp = 'acp',
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
  user_questions: boolean;
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

export type AgentDefinitionSource = 'builtin' | 'user_config' | 'workspace_config' | 'remote_registry';

/** One agent definition in the driver-era catalog. */
export interface AgentCatalogEntry {
  id: string;
  name: string;
  description?: string;
  driver_id: string;
  /** Driver protocol family: `acp` or `native`. */
  driver_protocol: string;
  enabled: boolean;
  available: boolean;
  capabilities: AgentCapabilities;
  source: AgentDefinitionSource;
  executable?: string;
}

export type DriverProtocol = 'acp' | 'native';

/** One registered driver. */
export interface DriverInfo {
  id: string;
  display_name: string;
  protocol: DriverProtocol;
  version?: string;
}

/** Declared, driver-supported and probe-verified capability sets. */
export interface ResolvedCapabilities {
  declared: AgentCapabilities;
  driver_supported: AgentCapabilities;
  verified: AgentCapabilities;
}

/** Result of probing an agent without creating a session. */
export interface AgentProbeResult {
  agent_id?: string;
  driver_id: string;
  protocol: DriverProtocol;
  protocol_version?: number;
  available: boolean;
  capabilities: ResolvedCapabilities;
  agent_info?: unknown;
  diagnostics?: string[];
}

export enum SessionStatus {
  Creating = 'creating',
  Starting = 'starting',
  Running = 'running',
  WaitingForInput = 'waiting_for_input',
  WaitingForPermission = 'waiting_for_permission',
  WaitingForQuestion = 'waiting_for_question',
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
  /**
   * Provider/session metadata. `restored: true` marks history that was rebuilt
   * from storage after a daemon restart; such sessions are read-only.
   */
  metadata?: AgentSessionMetadata | null;
  /**
   * The caller's presentation state. The admin attaches it per user; the
   * daemon never sends it, so it is absent from daemon payloads.
   */
  ui?: SessionUiState;
}

/** Per-user pins, archive state and labels for one session. */
export interface SessionUiState {
  pinned: boolean;
  /** Position within the pinned group; unranked pins sort last. */
  pinned_rank?: number;
  archived: boolean;
  /** Agent activity after the caller's last read. */
  unread: boolean;
  labels?: string[];
  folder_id?: string;
}

export interface AgentSessionMetadata {
  /** True when the session was restored from persisted history after a restart. */
  restored?: boolean;
  /** Model requested for the session, when the provider records it. */
  model?: string;
  [key: string]: unknown;
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

export interface AgentPermissionOption {
  option_id: string;
  name: string;
  /** Provider option kind, e.g. `allow_once`, `allow_always`, `reject_once`, `reject_always`. */
  kind: string;
}

export interface AgentPermissionRequest {
  id: string;
  session_id: SessionId;
  turn_id?: TurnId;
  tool_call_id?: string;
  tool_name: string;
  description: string;
  input: unknown;
  options?: AgentPermissionOption[];
  status: PermissionStatus;
  created_at: Timestamp;
  resolved_at?: Timestamp;
  expires_at?: Timestamp;
}

export interface PermissionDecision {
  permission_id: string;
  approve: boolean;
  option_id?: string;
  message?: string;
}

export interface RespondPermissionRequest {
  permission_id: string;
  approve: boolean;
  option_id?: string;
  message?: string;
}

export enum QuestionStatus {
  Pending = 'pending',
  Answered = 'answered',
  Rejected = 'rejected',
  TimedOut = 'timed_out',
  Cancelled = 'cancelled',
  DeliveryFailed = 'delivery_failed',
}

export interface AgentQuestionOption {
  label: string;
  description?: string;
}

export interface AgentQuestion {
  id: string;
  header?: string;
  question: string;
  options?: AgentQuestionOption[];
  multi_select?: boolean;
  allow_custom?: boolean;
  is_secret?: boolean;
}

export interface AgentQuestionRequest {
  id: string;
  session_id: SessionId;
  turn_id?: TurnId;
  tool_call_id?: string;
  questions: AgentQuestion[];
  answers?: AgentQuestionAnswer[];
  status: QuestionStatus;
  created_at: Timestamp;
  resolved_at?: Timestamp;
  expires_at?: Timestamp;
}

export interface AgentQuestionAnswer {
  id: string;
  selected: string[];
  custom?: string;
}

export interface RespondQuestionRequest {
  question_id: string;
  answers?: AgentQuestionAnswer[];
  reject?: boolean;
  message?: string;
}

export interface QuestionEventPayload {
  question: AgentQuestionRequest;
}

export const AGENT_EVENT_TYPES = [
  'session.created',
  'session.started',
  'session.resumed',
  'session.terminated',
  'session.deleted',
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
  'question.requested',
  'question.answered',
  'question.rejected',
  'agent.status.changed',
  'agent.plan.updated',
  'agent.usage.updated',
  'agent.activity.changed',
  'agent.notice',
  'agent.terminal.output',
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

export type PlanEntryStatus =
  | 'pending'
  | 'in_progress'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'unknown';

export type PlanEntryPriority = 'high' | 'medium' | 'low' | 'unknown';

/** One item of an agent plan. */
export interface AgentPlanEntry {
  /** Stable entry id when the agent provides one. */
  id?: string;
  /** Human-readable step. */
  content: string;
  status: PlanEntryStatus;
  priority: PlanEntryPriority;
}

/** An agent plan; plans are never mixed into messages. */
export interface AgentPlan {
  id: string;
  session_id: SessionId;
  turn_id?: TurnId;
  native_plan_id?: string;
  entries: AgentPlanEntry[];
  updated_at: Timestamp;
}

export interface PlanUpdatedPayload {
  plan: AgentPlan;
}

/**
 * Usage reported by the agent. Structured fields are present when Fleet
 * understands the protocol; the raw report is preserved for others.
 */
export interface UsageUpdatedPayload {
  used?: number;
  size?: number;
  cost?: number;
  raw?: unknown;
}

/** Foreground activity reported by an agent, independent of a Fleet turn. */
export interface ActivityChangedPayload {
  /** `running`, `requires_action`, `idle`, or `unknown`. */
  state: string;
  turn_id?: TurnId;
  /** Whether the agent started the activity without a user prompt. */
  agent_initiated: boolean;
  /** Background work kinds; omitted when the agent reported none. */
  background?: string[];
}

export interface NoticePayload {
  /** Notice kind, preserving the agent's discriminator. */
  kind: string;
  message: string;
  /** `info`, `warning`, `error`, or `unknown`. */
  level: string;
  raw?: unknown;
}

/**
 * Agent-owned terminal output, distinct from a Fleet-created terminal. These
 * terminals are display-only in the UI.
 */
export interface TerminalOutputPayload {
  terminal_id: string;
  /** Base64-encoded output bytes, exactly as the agent sent them. */
  data: string;
  /** Lossy UTF-8 rendering for display. */
  text: string;
  command?: string;
  cwd?: string;
  exit_code?: number;
  /** Whether this update marks the terminal as exited. */
  done: boolean;
  raw?: unknown;
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
  'session.deleted': unknown;
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
  'question.requested': QuestionEventPayload;
  'question.answered': QuestionEventPayload;
  'question.rejected': QuestionEventPayload;
  'agent.status.changed': StatusChangedPayload;
  'agent.plan.updated': PlanUpdatedPayload;
  'agent.usage.updated': UsageUpdatedPayload;
  'agent.activity.changed': ActivityChangedPayload;
  'agent.notice': NoticePayload;
  'agent.terminal.output': TerminalOutputPayload;
  'agent.raw_event': unknown;
}

export type TypedAgentEvent<T extends AgentEventType> = AgentEvent<AgentEventPayloadMap[T]>;

/** Response shape used by the session detail endpoint / RPC. */
export interface SessionDetail {
  session: AgentSession;
  turns: AgentTurn[];
  messages: AgentMessage[];
  tool_calls: AgentToolCall[];
  permissions: AgentPermissionRequest[];
  questions: AgentQuestionRequest[];
}

/** Browser request body for `POST /sessions`. */
export interface CreateAgentSessionRequest {
  /** Driver-era agent definition id. Preferred over `provider`. */
  agent_definition_id?: string;
  /** Legacy provider id, still accepted by the daemon. */
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

/** `CreateAgentSessionRequest` plus the machine routing field added by the agent admin. */
export interface CreateSessionRequest extends CreateAgentSessionRequest {
  machine_id: MachineId;
}

/** Browser request body for `PATCH /sessions/{id}`. */
export interface UpdateSessionRequest {
  /** New display title. `null` or an empty string clears the custom title. */
  title: string | null;
}

/** A partial update of the caller's presentation state for one session. */
export interface UpdateSessionUiRequest {
  pinned?: boolean;
  archived?: boolean;
  /** Replaces the whole label list; an empty list clears it. */
  labels?: string[];
  /** `null` removes the session from its folder. */
  folder_id?: string | null;
}

export interface BulkUpdateSessionUiRequest extends UpdateSessionUiRequest {
  ids: SessionId[];
}

export interface SessionFolder {
  id: string;
  name: string;
  sort_order: number;
}

/** Counts shown on the rail's filter chips. */
export interface SessionFacets {
  all: number;
  running: number;
  waiting: number;
  failed: number;
  closed: number;
  archived: number;
  unread: number;
  pinned: number;
}

/** One page of the session rail. */
export interface SessionPage {
  items: AgentSession[];
  next_cursor?: string;
}

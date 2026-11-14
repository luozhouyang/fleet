import type { EventId, MachineId, SessionId, Timestamp, WorkspaceId } from './common';
import type { AgentEvent } from './agent';
import type { MachineStatusEvent } from './machine';
import type { WorkspaceEvent } from './workspace';
import type { GitStatus } from './git';

/** Platform-level event forwarded by the agent admin to browsers. */
export interface PlatformEvent<P = unknown> {
  event_id: EventId;
  /** Monotonic sequence per machine connection, used for gap detection. */
  sequence: number;
  type: string;
  timestamp: Timestamp;
  machine_id?: MachineId;
  workspace_id?: WorkspaceId;
  session_id?: SessionId;
  payload: P;
}

export const PLATFORM_EVENT_TYPES = [
  'agent.event',
  'workspace.event',
  'git.status.changed',
  'machine.status',
  'connection.state',
  'platform.lagged',
] as const;

export type PlatformEventType = (typeof PLATFORM_EVENT_TYPES)[number];

export interface GitStatusChangedPayload {
  workspace_id: WorkspaceId;
  status: GitStatus;
}

export interface ConnectionStatePayload {
  machine_id: MachineId;
  connection_id: string;
  state: 'online' | 'offline';
}

export interface LagPayload {
  skipped: number;
  message?: string;
}

export interface PlatformEventPayloadMap {
  'agent.event': AgentEvent;
  'workspace.event': WorkspaceEvent;
  'git.status.changed': GitStatusChangedPayload;
  'machine.status': MachineStatusEvent;
  'connection.state': ConnectionStatePayload;
  'platform.lagged': LagPayload;
}

export type TypedPlatformEvent<T extends PlatformEventType> = PlatformEvent<
  PlatformEventPayloadMap[T]
>;

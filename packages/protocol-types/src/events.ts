import type { EventId, MachineId, SessionId, Timestamp, WorkspaceId } from './common';
import type { AgentEvent } from './agent';
import type { MachineStatusEvent } from './machine';
import type { WorkspaceEvent } from './workspace';
import type { GitStatus } from './git';

/** Platform-level event forwarded by the control plane to browsers. */
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
  'connection.closed',
] as const;

export type PlatformEventType = (typeof PLATFORM_EVENT_TYPES)[number];

export interface PlatformEventPayloadMap {
  'agent.event': AgentEvent;
  'workspace.event': WorkspaceEvent;
  'git.status.changed': GitStatus;
  'machine.status': MachineStatusEvent;
  'connection.closed': unknown;
}

export type TypedPlatformEvent<T extends PlatformEventType> = PlatformEvent<
  PlatformEventPayloadMap[T]
>;

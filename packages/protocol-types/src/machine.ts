import type { MachineId, Timestamp } from './common';
import type { ProviderInfo } from './agent';
import type { WorkspaceSummary } from './workspace';

export enum MachineStatus {
  Online = 'online',
  Offline = 'offline',
  Degraded = 'degraded',
  Unknown = 'unknown',
}

/** A machine registered with the agent admin. */
export interface Machine {
  id: MachineId;
  name: string;
  hostname: string;
  os: string;
  arch: string;
  daemon_version: string;
  protocol_version: number;
  status: MachineStatus;
  created_at: Timestamp;
  updated_at: Timestamp;
  connected_at?: Timestamp;
  last_seen_at?: Timestamp;
  /** Omitted by the server when empty. */
  providers?: ProviderInfo[];
  /** Omitted by the server when empty. */
  workspaces?: WorkspaceSummary[];
  workspace_count: number;
  public_key?: string;
}

/** Machine metadata used by the agent admin; not the wire `hello` payload. */
export interface MachineRegistration {
  machine_id: MachineId;
  name: string;
  hostname: string;
  os: string;
  arch: string;
  daemon_version: string;
  protocol_version: number;
  public_key: string;
  providers: ProviderInfo[];
  workspaces: WorkspaceSummary[];
}

/** Periodic liveness payload sent by the daemon. */
export interface HeartbeatPayload {
  sent_at: Timestamp;
  sequence: number;
  uptime_secs: number;
  active_sessions: number;
  status?: MachineStatus;
}

/** Health/version information exposed by the daemon. */
export interface HealthPayload {
  machine_id: string;
  daemon_version: string;
  protocol_version: number;
  uptime_secs: number;
  active_sessions: number;
  timestamp: Timestamp;
}

/**
 * Payload of the browser event with type `machine.status`.
 *
 * The agent admin publishes the full machine snapshot, not a compact
 * status delta, so consumers can refresh name/providers/workspaces too.
 */
export type MachineStatusEvent = Machine;

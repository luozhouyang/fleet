/**
 * Shared primitives mirrored from `crates/core-common`.
 *
 * IDs are opaque strings. Timestamps are RFC 3339 strings on the wire.
 */

export type MachineId = string;
export type WorkspaceId = string;
export type ProjectId = string;
export type SessionId = string;
export type TurnId = string;
export type EventId = string;
export type MessageId = string;
export type RequestId = string;
export type StreamId = string;
export type Timestamp = string;

export interface ApiError {
  code: string;
  message: string;
  details?: unknown;
}

export interface Ack {
  ok: boolean;
}

export interface Page<T> {
  items: T[];
  next_cursor?: string;
}

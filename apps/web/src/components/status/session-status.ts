import { SessionStatus, type AgentSession } from '@agent-fleet/protocol-types';
import type { Tone } from '@agent-fleet/ui';

export const sessionTones: Record<SessionStatus, Tone> = {
  [SessionStatus.Creating]: 'slate',
  [SessionStatus.Starting]: 'teal',
  [SessionStatus.Running]: 'teal',
  [SessionStatus.WaitingForInput]: 'amber',
  [SessionStatus.WaitingForPermission]: 'amber',
  [SessionStatus.Interrupted]: 'amber',
  [SessionStatus.Completed]: 'green',
  [SessionStatus.Failed]: 'red',
  [SessionStatus.Terminated]: 'slate',
};

export const sessionLabels: Record<SessionStatus, string> = {
  [SessionStatus.Creating]: 'Creating',
  [SessionStatus.Starting]: 'Starting',
  [SessionStatus.Running]: 'Running',
  [SessionStatus.WaitingForInput]: 'Waiting for input',
  [SessionStatus.WaitingForPermission]: 'Waiting for permission',
  [SessionStatus.Interrupted]: 'Interrupted',
  [SessionStatus.Completed]: 'Completed',
  [SessionStatus.Failed]: 'Failed',
  [SessionStatus.Terminated]: 'Terminated',
};

export const activeSessionStatuses = new Set([
  SessionStatus.Creating,
  SessionStatus.Starting,
  SessionStatus.Running,
  SessionStatus.WaitingForInput,
  SessionStatus.WaitingForPermission,
]);

export function sessionStatusTone(status: SessionStatus | undefined): Tone {
  return status ? sessionTones[status] : 'neutral';
}

export function isSessionActive(status: SessionStatus | undefined): boolean {
  return status !== undefined && activeSessionStatuses.has(status);
}

/** Short, human-friendly label for compact navigation lists. */
export function sessionLabel(session: AgentSession): string {
  if (session.title) return session.title;
  const raw = session.id.startsWith('session_')
    ? session.id.slice('session_'.length)
    : session.id;
  return raw.length > 10 ? `${raw.slice(0, 10)}…` : raw;
}

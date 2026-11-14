import { MachineStatus, SessionStatus } from '@agent-fleet/protocol-types';

export type StatusTone = 'running' | 'waiting' | 'failed' | 'idle' | 'offline';

export interface StatusMeta {
  tone: StatusTone;
  label: string;
}

const SESSION_STATUS: Record<SessionStatus, StatusMeta> = {
  [SessionStatus.Creating]: { tone: 'idle', label: 'Creating' },
  [SessionStatus.Starting]: { tone: 'waiting', label: 'Starting' },
  [SessionStatus.Running]: { tone: 'running', label: 'Running' },
  [SessionStatus.WaitingForInput]: { tone: 'waiting', label: 'Waiting' },
  [SessionStatus.WaitingForPermission]: { tone: 'waiting', label: 'Waiting' },
  [SessionStatus.Interrupted]: { tone: 'idle', label: 'Interrupted' },
  [SessionStatus.Completed]: { tone: 'idle', label: 'Done' },
  [SessionStatus.Failed]: { tone: 'failed', label: 'Failed' },
  [SessionStatus.Terminated]: { tone: 'offline', label: 'Closed' },
};

export function sessionStatusMeta(status?: SessionStatus | null): StatusMeta {
  if (!status) return { tone: 'idle', label: 'Unknown' };
  return SESSION_STATUS[status] ?? { tone: 'idle', label: status };
}

const MACHINE_STATUS: Record<MachineStatus, StatusMeta> = {
  [MachineStatus.Online]: { tone: 'running', label: 'Online' },
  [MachineStatus.Offline]: { tone: 'offline', label: 'Offline' },
  [MachineStatus.Degraded]: { tone: 'waiting', label: 'Degraded' },
  [MachineStatus.Unknown]: { tone: 'idle', label: 'Unknown' },
};

export function machineStatusMeta(status?: MachineStatus | null): StatusMeta {
  if (!status) return { tone: 'idle', label: 'Unknown' };
  return MACHINE_STATUS[status] ?? { tone: 'idle', label: status };
}

export function sessionIsActive(status?: SessionStatus | null): boolean {
  if (!status) return false;
  return ![
    SessionStatus.Completed,
    SessionStatus.Failed,
    SessionStatus.Terminated,
    SessionStatus.Interrupted,
  ].includes(status);
}

import {
  GitFileStatus,
  MachineStatus,
  PermissionStatus,
  SessionStatus,
  ToolCallStatus,
} from '@agent-fleet/protocol-types';
import { Badge, StatusDot, type Tone } from '@agent-fleet/ui';
import {
  activeSessionStatuses,
  sessionLabels,
  sessionTones,
} from './session-status';

export function SessionStatusBadge({ status }: { status: SessionStatus | undefined }) {
  if (!status) return <Badge tone="slate">Unknown</Badge>;
  return (
    <Badge tone={sessionTones[status]}>
      <StatusDot tone={sessionTones[status]} pulse={activeSessionStatuses.has(status)} />
      {sessionLabels[status]}
    </Badge>
  );
}

const machineTones: Record<MachineStatus, Tone> = {
  [MachineStatus.Online]: 'green',
  [MachineStatus.Degraded]: 'amber',
  [MachineStatus.Offline]: 'slate',
  [MachineStatus.Unknown]: 'neutral',
};

const machineLabels: Record<MachineStatus, string> = {
  [MachineStatus.Online]: 'Online',
  [MachineStatus.Degraded]: 'Degraded',
  [MachineStatus.Offline]: 'Offline',
  [MachineStatus.Unknown]: 'Unknown',
};

export function MachineStatusBadge({ status }: { status: MachineStatus }) {
  return (
    <Badge tone={machineTones[status]}>
      <StatusDot
        tone={machineTones[status]}
        pulse={status === MachineStatus.Online || status === MachineStatus.Degraded}
      />
      {machineLabels[status]}
    </Badge>
  );
}

const permissionTones: Record<PermissionStatus, Tone> = {
  [PermissionStatus.Pending]: 'amber',
  [PermissionStatus.Approved]: 'green',
  [PermissionStatus.Rejected]: 'red',
  [PermissionStatus.TimedOut]: 'amber',
  [PermissionStatus.Cancelled]: 'slate',
  [PermissionStatus.DeliveryFailed]: 'red',
};

const permissionLabels: Record<PermissionStatus, string> = {
  [PermissionStatus.Pending]: 'Pending',
  [PermissionStatus.Approved]: 'Approved',
  [PermissionStatus.Rejected]: 'Rejected',
  [PermissionStatus.TimedOut]: 'Timed out',
  [PermissionStatus.Cancelled]: 'Cancelled',
  [PermissionStatus.DeliveryFailed]: 'Delivery failed',
};

export function PermissionStatusBadge({ status }: { status: PermissionStatus }) {
  return <Badge tone={permissionTones[status]}>{permissionLabels[status]}</Badge>;
}

const toolTones: Record<ToolCallStatus, Tone> = {
  [ToolCallStatus.Pending]: 'slate',
  [ToolCallStatus.Running]: 'teal',
  [ToolCallStatus.Completed]: 'green',
  [ToolCallStatus.Failed]: 'red',
  [ToolCallStatus.Cancelled]: 'slate',
};

export function ToolStatusBadge({ status }: { status: ToolCallStatus }) {
  return (
    <Badge tone={toolTones[status]}>
      {status.replaceAll('_', ' ')}
    </Badge>
  );
}

const gitTones: Record<GitFileStatus, Tone> = {
  [GitFileStatus.Modified]: 'amber',
  [GitFileStatus.Added]: 'green',
  [GitFileStatus.Deleted]: 'red',
  [GitFileStatus.Renamed]: 'teal',
  [GitFileStatus.Copied]: 'teal',
  [GitFileStatus.Untracked]: 'green',
  [GitFileStatus.Conflicted]: 'red',
  [GitFileStatus.Ignored]: 'slate',
  [GitFileStatus.Unmodified]: 'slate',
};

const gitLetters: Record<GitFileStatus, string> = {
  [GitFileStatus.Modified]: 'M',
  [GitFileStatus.Added]: 'A',
  [GitFileStatus.Deleted]: 'D',
  [GitFileStatus.Renamed]: 'R',
  [GitFileStatus.Copied]: 'C',
  [GitFileStatus.Untracked]: 'U',
  [GitFileStatus.Conflicted]: '!',
  [GitFileStatus.Ignored]: 'I',
  [GitFileStatus.Unmodified]: '',
};

export function GitStatusLetter({ status }: { status: GitFileStatus }) {
  return (
    <Badge tone={gitTones[status]} className="w-5 justify-center px-0 text-[10px]">
      {gitLetters[status]}
    </Badge>
  );
}

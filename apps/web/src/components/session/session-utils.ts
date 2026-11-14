import {
  PermissionStatus,
  ToolCallStatus,
  type AgentMessage,
  type AgentPermissionRequest,
  type AgentSession,
} from '@agent-fleet/protocol-types';
import type { StatusTone } from '@/lib/status';
import { shortId } from '@/lib/utils/format';

export interface StatusLabel {
  label: string;
  tone: StatusTone;
}

export function sessionTitle(session: AgentSession | null | undefined): string {
  if (!session) return 'Session';
  const title = session.title?.trim();
  return title || `Session ${shortId(session.id, 10)}`;
}

export function sessionModel(session: AgentSession | null | undefined): string | undefined {
  const metadata = session?.metadata;
  if (!metadata || typeof metadata !== 'object') return undefined;
  const model = (metadata as { model?: unknown }).model;
  return typeof model === 'string' && model.trim() ? model : undefined;
}

function truncate(text: string, maxLength: number): string {
  return text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
}

export function compactJson(value: unknown, maxLength = 320): string {
  if (value == null) return '';
  if (typeof value === 'string') return truncate(value, maxLength);
  try {
    const text = JSON.stringify(value);
    return text === undefined ? truncate(String(value), maxLength) : truncate(text, maxLength);
  } catch {
    return truncate(String(value), maxLength);
  }
}

export function prettyJson(value: unknown, maxLength = 20_000): string {
  if (value == null) return '';
  try {
    const text = JSON.stringify(value, null, 2);
    return text === undefined ? truncate(String(value), maxLength) : truncate(text, maxLength);
  } catch {
    return truncate(String(value), maxLength);
  }
}

export function permissionPreview(permission: AgentPermissionRequest): string {
  const input = permission.input;
  if (typeof input === 'string' && input.trim()) return truncate(input.trim(), 800);
  if (input && typeof input === 'object') {
    const record = input as Record<string, unknown>;
    for (const key of ['command', 'cmd', 'script', 'path', 'file_path', 'pattern', 'url']) {
      const value = record[key];
      if (typeof value === 'string' && value.trim()) return truncate(value.trim(), 800);
    }
  }
  return compactJson(input, 800);
}

export function permissionFilePaths(permission: AgentPermissionRequest): string[] {
  const input = permission.input;
  if (!input || typeof input !== 'object') return [];
  const record = input as Record<string, unknown>;
  const paths = new Set<string>();
  const add = (value: unknown) => {
    if (typeof value === 'string' && value.trim()) paths.add(value.trim());
  };
  for (const key of ['path', 'file_path', 'filePath', 'filename', 'file']) {
    add(record[key]);
  }
  for (const key of ['paths', 'files', 'file_paths']) {
    const value = record[key];
    if (Array.isArray(value)) {
      for (const item of value) {
        if (typeof item === 'string') add(item);
        else if (item && typeof item === 'object') add((item as Record<string, unknown>).path);
      }
    }
  }
  return [...paths].slice(0, 8);
}

export function permissionRisk(toolName: string): string {
  const name = toolName.toLowerCase();
  if (
    name.includes('bash') ||
    name.includes('shell') ||
    name.includes('exec') ||
    name.includes('command')
  ) {
    return 'Runs a command on the workspace machine with this session’s permissions. Review it before continuing.';
  }
  if (
    name.includes('write') ||
    name.includes('edit') ||
    name.includes('patch') ||
    name.includes('create')
  ) {
    return 'Writes to files in the workspace. You can review the result with version control.';
  }
  if (name.includes('delete') || name.includes('remove')) {
    return 'Deletes files or resources. This may not be reversible.';
  }
  return 'The agent will act on your machine with the permissions granted to this session.';
}

const PERMISSION_STATUS: Record<PermissionStatus, StatusLabel> = {
  [PermissionStatus.Pending]: { label: 'Pending', tone: 'waiting' },
  [PermissionStatus.Approved]: { label: 'Approved', tone: 'running' },
  [PermissionStatus.Rejected]: { label: 'Rejected', tone: 'failed' },
  [PermissionStatus.TimedOut]: { label: 'Timed out', tone: 'waiting' },
  [PermissionStatus.Cancelled]: { label: 'Cancelled', tone: 'idle' },
  [PermissionStatus.DeliveryFailed]: { label: 'Delivery failed', tone: 'failed' },
};

export function permissionStatusMeta(status: PermissionStatus): StatusLabel {
  return PERMISSION_STATUS[status] ?? { label: status, tone: 'idle' };
}

const TOOL_STATUS: Record<ToolCallStatus, StatusLabel> = {
  [ToolCallStatus.Pending]: { label: 'Pending', tone: 'waiting' },
  [ToolCallStatus.Running]: { label: 'Running', tone: 'running' },
  [ToolCallStatus.Completed]: { label: 'Completed', tone: 'running' },
  [ToolCallStatus.Failed]: { label: 'Failed', tone: 'failed' },
  [ToolCallStatus.Cancelled]: { label: 'Cancelled', tone: 'idle' },
};

export function toolStatusMeta(status: ToolCallStatus): StatusLabel {
  return TOOL_STATUS[status] ?? { label: status, tone: 'idle' };
}

export function isAgentSession(value: unknown): value is AgentSession {
  return (
    typeof value === 'object' &&
    value !== null &&
    'id' in value &&
    'status' in value &&
    'provider' in value
  );
}

export function isAgentMessage(value: unknown): value is AgentMessage {
  return (
    typeof value === 'object' &&
    value !== null &&
    'id' in value &&
    'role' in value &&
    'content' in value
  );
}

export function localMessageId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return `local_${crypto.randomUUID()}`;
  }
  return `local_${Date.now().toString(36)}`;
}

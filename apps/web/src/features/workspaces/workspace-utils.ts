import {
  DiffLineType,
  GitFileStatus,
  SessionStatus,
  type AgentSession,
  type FileDiff,
} from '@agent-fleet/protocol-types';
import { sessionIsActive, type StatusTone } from '@/lib/status';

export function gitStatusLetter(status: GitFileStatus): string {
  switch (status) {
    case GitFileStatus.Added:
      return 'A';
    case GitFileStatus.Deleted:
      return 'D';
    case GitFileStatus.Renamed:
      return 'R';
    case GitFileStatus.Copied:
      return 'C';
    case GitFileStatus.Untracked:
      return 'U';
    case GitFileStatus.Conflicted:
      return '!';
    case GitFileStatus.Ignored:
      return 'I';
    case GitFileStatus.Unmodified:
      return ' ';
    case GitFileStatus.Modified:
    default:
      return 'M';
  }
}

export function gitStatusClass(status: GitFileStatus): string {
  switch (status) {
    case GitFileStatus.Added:
    case GitFileStatus.Untracked:
      return 'text-success';
    case GitFileStatus.Deleted:
    case GitFileStatus.Conflicted:
      return 'text-destructive';
    case GitFileStatus.Renamed:
    case GitFileStatus.Copied:
      return 'text-primary';
    case GitFileStatus.Ignored:
    case GitFileStatus.Unmodified:
      return 'text-faint';
    case GitFileStatus.Modified:
    default:
      return 'text-warning';
  }
}

export interface DiffSides {
  original: string;
  modified: string;
}

/**
 * Turns a unified diff into the two sides Monaco needs. Hunk metadata is not a
 * source line, so it is intentionally omitted; unchanged regions outside the
 * hunks are also omitted.
 */
export function buildDiffSides(file: FileDiff | undefined): DiffSides {
  const original: string[] = [];
  const modified: string[] = [];

  for (const hunk of file?.hunks ?? []) {
    for (const line of hunk.lines) {
      if (line.line_type === DiffLineType.Context) {
        original.push(line.content);
        modified.push(line.content);
      } else if (line.line_type === DiffLineType.Deleted) {
        original.push(line.content);
      } else if (line.line_type === DiffLineType.Added) {
        modified.push(line.content);
      }
    }
  }

  return { original: original.join('\n'), modified: modified.join('\n') };
}

export interface SessionSummary {
  tone: StatusTone;
  label: string;
}

export function workspaceSessionSummary(sessions: AgentSession[]): SessionSummary {
  if (sessions.length === 0) return { tone: 'idle', label: 'idle' };

  const active = sessions.filter((session) => sessionIsActive(session.status));
  if (active.length > 0) {
    const waiting = active.some(
      (session) =>
        session.status === SessionStatus.Starting ||
        session.status === SessionStatus.WaitingForInput ||
        session.status === SessionStatus.WaitingForPermission ||
        session.status === SessionStatus.WaitingForQuestion,
    );
    return {
      tone: waiting ? 'waiting' : 'running',
      label: `${active.length} active`,
    };
  }

  return {
    tone: 'idle',
    label: `${sessions.length} ${sessions.length === 1 ? 'session' : 'sessions'}`,
  };
}

export function latestSessionActivity(sessions: AgentSession[]): string | undefined {
  let latest: string | undefined;
  let latestTime = Number.NEGATIVE_INFINITY;

  for (const session of sessions) {
    const time = Date.parse(session.updated_at);
    if (!Number.isNaN(time) && time > latestTime) {
      latestTime = time;
      latest = session.updated_at;
    }
  }

  return latest;
}

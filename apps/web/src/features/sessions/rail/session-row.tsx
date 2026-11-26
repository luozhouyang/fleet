import type { AgentSession } from '@agent-fleet/protocol-types';
import { Link } from '@tanstack/react-router';
import { memo, type MouseEvent } from 'react';
import { Kbd } from '@/components/ui/kbd';
import { formatRelativeTime } from '@/lib/utils/format';
import { cn } from '@/lib/utils/cn';
import { attentionLevel } from '../rail-model';
import { StatusGlyph } from './status-glyph';

export interface SessionRowProps {
  session: AgentSession;
  selected: boolean;
  /** Machine and workspace display names for the subtitle. */
  machineName: string;
  workspaceName?: string;
  offline: boolean;
  /** Multi-select state; selected rows show a check affordance. */
  multiSelected: boolean;
  onActivate: (session: AgentSession, event: MouseEvent<HTMLAnchorElement>) => void;
  onContextMenu: (session: AgentSession, event: MouseEvent<HTMLAnchorElement>) => void;
}

/**
 * One session in the rail.
 *
 * Memoized: a streaming delta updates a session's `ui.unread` or status and
 * must repaint only that row, not the whole list.
 */
export const SessionRow = memo(function SessionRow({
  session,
  selected,
  machineName,
  workspaceName,
  offline,
  multiSelected,
  onActivate,
  onContextMenu,
}: SessionRowProps) {
  const unread = Boolean(session.ui?.unread);
  const level = attentionLevel(session);
  const title = session.title?.trim() || `Session ${session.id.slice(0, 10)}`;

  return (
    <Link
      to="/sessions/$sessionId"
      params={{ sessionId: session.id }}
      onClick={(event) => onActivate(session, event)}
      onContextMenu={(event) => {
        event.preventDefault();
        onContextMenu(session, event);
      }}
      aria-current={selected ? 'page' : undefined}
      data-session-id={session.id}
      data-unread={unread || undefined}
      className={cn(
        'group flex h-[46px] items-center gap-2.5 rounded-lg border border-transparent px-2 outline-none transition-colors',
        'hover:bg-muted focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/20',
        selected && 'border-border bg-selected',
        multiSelected && 'border-primary/60 bg-primary-soft',
      )}
    >
      <StatusGlyph session={session} unread={unread} />

      <span className="min-w-0 flex-1">
        <span
          className={cn(
            'block truncate text-[12px] leading-4',
            unread ? 'font-bold text-foreground' : 'font-medium text-body',
          )}
        >
          {title}
        </span>
        <span className="flex min-w-0 items-center gap-1 font-mono text-[9px] leading-3 text-faint">
          <span className="truncate">{machineName}</span>
          {workspaceName ? (
            <>
              <span aria-hidden="true">·</span>
              <span className="truncate">{workspaceName}</span>
            </>
          ) : null}
          {offline ? (
            <>
              <span aria-hidden="true">·</span>
              <span className="shrink-0 text-warning">offline</span>
            </>
          ) : null}
        </span>
      </span>

      <span className="flex shrink-0 flex-col items-end gap-0.5">
        <span className="font-mono text-[9px] leading-3 text-faint">
          {formatRelativeTime(session.updated_at)}
        </span>
        {level === 'needs-you' ? (
          <span className="font-mono text-[8.5px] font-bold uppercase leading-3 tracking-[0.06em] text-status-attention">
            Needs you
          </span>
        ) : session.ui?.labels?.length ? null : (
          <Kbd className="hidden group-hover:inline-flex">↵</Kbd>
        )}
      </span>

      {session.ui?.labels?.length ? (
        <span className="sr-only">{session.ui.labels.join(', ')}</span>
      ) : null}
    </Link>
  );
});

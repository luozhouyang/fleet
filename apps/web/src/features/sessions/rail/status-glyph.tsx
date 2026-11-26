import { SessionStatus } from '@agent-fleet/protocol-types';
import { CircleAlert, CircleCheck, CircleDot, CirclePause, CircleX, Loader } from 'lucide-react';
import { attentionLevel, type AttentionLevel } from '../rail-model';
import type { AgentSession } from '@agent-fleet/protocol-types';
import { cn } from '@/lib/utils/cn';

/**
 * The rail's status lane glyph.
 *
 * Shape and color both carry the state so it reads without color vision:
 * spinning for work, bell-like alert for "needs you", cross for failed,
 * check for done.
 */
export function StatusGlyph({
  session,
  unread,
  className,
}: {
  session: AgentSession;
  unread: boolean;
  className?: string;
}) {
  const level = attentionLevel(session);
  const { icon: Icon, tone, spin } = glyphFor(session, level);
  return (
    <span
      className={cn('relative inline-flex size-4 shrink-0 items-center justify-center', className)}
    >
      <Icon
        aria-hidden="true"
        className={cn(
          'size-3.5',
          tone,
          spin && 'motion-safe:animate-spin [animation-duration:2.4s]',
        )}
      />
      {unread ? (
        <span
          aria-hidden="true"
          className="absolute -bottom-0.5 -right-0.5 size-1.5 rounded-full bg-unread ring-2 ring-panel"
        />
      ) : null}
    </span>
  );
}

function glyphFor(
  session: AgentSession,
  level: AttentionLevel,
): { icon: typeof CircleDot; tone: string; spin: boolean } {
  if (level === 'needs-you') {
    return { icon: CircleAlert, tone: 'text-status-attention', spin: false };
  }
  if (level === 'failed') {
    return { icon: CircleX, tone: 'text-status-failed', spin: false };
  }
  switch (session.status) {
    case SessionStatus.Running:
    case SessionStatus.Starting:
      return { icon: Loader, tone: 'text-status-running', spin: true };
    case SessionStatus.Creating:
      return { icon: CircleDot, tone: 'text-status-idle', spin: false };
    case SessionStatus.Interrupted:
      return { icon: CirclePause, tone: 'text-status-idle', spin: false };
    case SessionStatus.Completed:
      return { icon: CircleCheck, tone: 'text-status-running', spin: false };
    case SessionStatus.Terminated:
      return { icon: CircleX, tone: 'text-status-idle', spin: false };
    default:
      return { icon: CircleDot, tone: 'text-status-waiting', spin: false };
  }
}

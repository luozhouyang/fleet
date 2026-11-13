import { MessageRole, type AgentMessage } from '@agent-fleet/protocol-types';
import { cn } from '@agent-fleet/ui';
import { formatTime } from '@/lib/utils/format';

export function MessageBubble({
  message,
  streaming = false,
}: {
  message: AgentMessage;
  streaming?: boolean;
}) {
  if (message.role === MessageRole.System) {
    return (
      <div className="flex justify-center py-1">
        <div className="max-w-2xl rounded-full border border-slate-800 bg-slate-900/70 px-3 py-1 text-center text-[11px] text-slate-500">
          {message.content}
        </div>
      </div>
    );
  }

  const isUser = message.role === MessageRole.User;

  return (
    <div className={cn('flex gap-3', isUser ? 'justify-end' : 'justify-start')}>
      {!isUser ? (
        <div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md border border-teal-500/20 bg-teal-500/10 text-[10px] font-bold text-teal-300">
          AF
        </div>
      ) : null}
      <div className={cn('max-w-[min(46rem,88%)] space-y-1.5', isUser && 'items-end')}>
        <div
          className={cn(
            'flex items-center gap-2 text-[10px] uppercase tracking-wider',
            isUser ? 'justify-end text-slate-500' : 'text-slate-500',
          )}
        >
          <span>{isUser ? 'You' : 'Assistant'}</span>
          <span>{formatTime(message.created_at)}</span>
          {message.id.startsWith('local_') ? (
            <span className="normal-case tracking-normal text-slate-600">sending</span>
          ) : null}
        </div>
        <div
          className={cn(
            'whitespace-pre-wrap break-words rounded-lg px-3.5 py-2.5 text-sm leading-6',
            isUser
              ? 'border border-teal-500/20 bg-teal-500/10 text-slate-100'
              : 'border border-slate-800 bg-slate-900/70 text-slate-200',
          )}
        >
          {message.content || (
            <span className="italic text-slate-500">Waiting for output…</span>
          )}
          {streaming ? (
            <span className="ml-1 inline-block h-4 w-1.5 animate-pulse rounded-sm bg-teal-400/80 align-middle" />
          ) : null}
        </div>
      </div>
    </div>
  );
}

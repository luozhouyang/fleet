import { MessageRole } from '@agent-fleet/protocol-types';
import { memo } from 'react';
import { useSessionStore } from '@/lib/session-store';
import { formatClock } from '@/lib/utils/format';
import { Markdown } from './markdown';

/**
 * One message in the transcript.
 *
 * The row resolves its own message from the store so a streaming delta only
 * re-renders the row that changed; the parent re-render is memoized away.
 */
export const MessageRow = memo(function MessageRow({
  sessionId,
  messageId,
  streaming,
}: {
  sessionId: string;
  messageId: string;
  streaming: boolean;
}) {
  const message = useSessionStore((state) => state.sessions[sessionId]?.messages[messageId]);
  if (!message) return null;

  if (message.role === MessageRole.User) {
    return (
      <div className="flex justify-end pb-4">
        <div className="min-w-0 max-w-[85%]">
          <div className="whitespace-pre-wrap break-words rounded-2xl rounded-br-md border border-chat-user-border bg-chat-user px-3.5 py-2.5 text-[12.5px] leading-[19px] text-foreground">
            {message.content}
          </div>
          <p className="mt-1 text-right font-mono text-[9px] text-faint">
            {formatClock(message.created_at)}
          </p>
        </div>
      </div>
    );
  }

  if (message.role === MessageRole.System) {
    return (
      <p className="whitespace-pre-wrap break-words pb-4 text-[11.5px] leading-[18px] text-muted-foreground">
        {message.content}
      </p>
    );
  }

  // While the agent is still writing, plain text is both cheaper and less
  // jumpy than re-parsing half-finished markdown on every delta. The finished
  // message switches to the full renderer.
  if (streaming) {
    return (
      <div className="pb-4 text-[12.5px] leading-[20px] text-body">
        <p className="whitespace-pre-wrap break-words">
          {message.content}
          <span
            aria-hidden="true"
            className="ml-0.5 inline-block h-[13px] w-[5px] translate-y-[2px] animate-pulse rounded-sm bg-primary motion-reduce:animate-none"
          />
        </p>
      </div>
    );
  }

  return (
    <div className="pb-4">
      <Markdown content={message.content} streaming={false} />
    </div>
  );
});

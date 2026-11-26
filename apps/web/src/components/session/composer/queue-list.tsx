import { ArrowDown, ArrowUp, X } from 'lucide-react';
import type { ReactNode } from 'react';
import type { QueuedPrompt } from '@/lib/session/composer-model';
import { cn } from '@/lib/utils/cn';

/**
 * Prompts waiting for the running turn to finish.
 *
 * The queue is per tab and not persisted: a prompt contains text the user has
 * not sent yet, and silently resending it after a reload would be surprising.
 */
export function QueueList({
  queue,
  paused,
  onMove,
  onRemove,
  onRetry,
}: {
  queue: QueuedPrompt[];
  /** A flush failed; nothing sends until the user retries. */
  paused: boolean;
  onMove: (id: string, direction: -1 | 1) => void;
  onRemove: (id: string) => void;
  onRetry: () => void;
}) {
  if (queue.length === 0) return null;
  return (
    <div className="mb-1.5 rounded-lg border border-border bg-card/70 px-2 py-1.5">
      <div className="mb-1 flex items-center gap-2">
        <p className="font-mono text-[9px] uppercase tracking-[0.12em] text-faint">
          Queued · {queue.length}
        </p>
        {paused ? (
          <>
            <span className="text-[10px] text-warning">Sending failed.</span>
            <button
              type="button"
              onClick={onRetry}
              className="text-[10px] font-bold text-primary underline underline-offset-2"
            >
              Retry
            </button>
          </>
        ) : null}
      </div>
      <ol className="flex flex-col gap-0.5">
        {queue.map((item, index) => (
          <li key={item.id} className="group/queued flex items-center gap-1.5">
            <span className="w-4 shrink-0 text-right font-mono text-[9px] text-faint">
              {index + 1}
            </span>
            <span className="min-w-0 flex-1 truncate text-[11px] text-body">
              {item.content || `${item.attachments.length} attachment(s)`}
            </span>
            <span className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover/queued:opacity-100 focus-within:opacity-100">
              <QueueButton
                label="Move up"
                disabled={index === 0}
                onClick={() => onMove(item.id, -1)}
              >
                <ArrowUp className="size-3" />
              </QueueButton>
              <QueueButton
                label="Move down"
                disabled={index === queue.length - 1}
                onClick={() => onMove(item.id, 1)}
              >
                <ArrowDown className="size-3" />
              </QueueButton>
              <QueueButton label="Remove from queue" onClick={() => onRemove(item.id)}>
                <X className="size-3" />
              </QueueButton>
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

function QueueButton({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        'flex size-4 items-center justify-center rounded text-faint hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
        disabled && 'pointer-events-none opacity-30',
      )}
    >
      {children}
    </button>
  );
}

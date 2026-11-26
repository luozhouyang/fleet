import { FileText, X } from 'lucide-react';
import { cn } from '@/lib/utils/cn';
import { formatBytes } from '@/lib/utils/format';
import type { PendingAttachment } from '@/lib/session/composer-model';

/** Thumbnails for the attachments waiting on the next message. */
export function AttachmentTray({
  attachments,
  onRemove,
}: {
  attachments: PendingAttachment[];
  onRemove: (id: string) => void;
}) {
  if (attachments.length === 0) return null;
  return (
    <ul className="mb-1.5 flex flex-wrap gap-1.5" aria-label="Attachments">
      {attachments.map((attachment) => (
        <li
          key={attachment.id}
          className="group/attachment relative flex items-center gap-1.5 rounded-md border border-border bg-card py-1 pl-1 pr-1.5"
        >
          {attachment.previewUrl ? (
            <img
              src={attachment.previewUrl}
              alt=""
              className="size-8 rounded object-cover"
              draggable={false}
            />
          ) : (
            <span className="flex size-8 items-center justify-center rounded bg-muted">
              <FileText className="size-3.5 text-muted-foreground" aria-hidden="true" />
            </span>
          )}
          <span className="min-w-0 max-w-[140px]">
            <span className="block truncate text-[10.5px] font-medium text-foreground">
              {attachment.name}
            </span>
            <span className="block font-mono text-[9px] text-faint">
              {formatBytes(attachment.size)}
            </span>
          </span>
          <button
            type="button"
            aria-label={`Remove ${attachment.name}`}
            onClick={() => onRemove(attachment.id)}
            className={cn(
              'ml-0.5 flex size-4 shrink-0 items-center justify-center rounded text-faint transition-colors',
              'hover:bg-destructive-soft hover:text-destructive focus-visible:bg-destructive-soft focus-visible:text-destructive focus-visible:outline-none',
            )}
          >
            <X className="size-3" />
          </button>
        </li>
      ))}
    </ul>
  );
}

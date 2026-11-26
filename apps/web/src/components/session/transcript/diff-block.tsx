import { cn } from '@/lib/utils/cn';
import type { InlineDiff } from '@/lib/session/tool-view';

/**
 * A bounded unified diff rendered as plain text.
 *
 * The content is untrusted agent output, so every line is a text node; no
 * syntax highlighting and no HTML interpretation happens here.
 */
export function DiffBlock({ diff }: { diff: InlineDiff }) {
  return (
    <div className="overflow-hidden rounded-md border border-code-border bg-code-bg">
      {diff.path ? (
        <div className="border-b border-code-border px-2 py-1 font-mono text-[9.5px] text-muted-foreground">
          {diff.path}
        </div>
      ) : null}
      <div className="max-h-72 overflow-auto py-1 font-mono text-[10.5px] leading-[16px]">
        {diff.lines.map((line, index) => (
          <div
            key={index}
            className={cn(
              'whitespace-pre px-2 text-code-fg',
              line.kind === 'add' && 'bg-diff-add-bg text-diff-add-fg',
              line.kind === 'remove' && 'bg-diff-del-bg text-diff-del-fg',
              line.kind === 'header' && 'text-faint',
            )}
          >
            <span aria-hidden="true" className="mr-1.5 select-none opacity-60">
              {line.kind === 'add' ? '+' : line.kind === 'remove' ? '-' : ' '}
            </span>
            {line.text}
          </div>
        ))}
      </div>
      {diff.truncated ? (
        <p className="border-t border-code-border px-2 py-1 text-[9.5px] text-faint">
          Diff truncated for display.
        </p>
      ) : null}
    </div>
  );
}

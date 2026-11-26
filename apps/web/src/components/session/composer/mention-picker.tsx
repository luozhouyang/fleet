import { FileText } from 'lucide-react';
import { cn } from '@/lib/utils/cn';

/**
 * `@path` suggestions above the composer.
 *
 * Paths come from the workspace search index, which already applies the
 * ignore rules, so a mention cannot advertise a build artifact as a source
 * file. The picker is keyboard-navigable and never steals focus.
 */
export function MentionPicker({
  paths,
  activeIndex,
  loading,
  onSelect,
}: {
  paths: string[];
  activeIndex: number;
  loading: boolean;
  onSelect: (path: string) => void;
}) {
  if (!loading && paths.length === 0) return null;
  return (
    <div
      role="listbox"
      aria-label="Workspace files"
      className="absolute inset-x-0 bottom-full z-20 mb-1.5 max-h-56 overflow-auto rounded-lg border border-border bg-card py-1 shadow-floating"
    >
      {loading && paths.length === 0 ? (
        <p className="px-3 py-1.5 text-[10.5px] text-faint">Searching workspace…</p>
      ) : null}
      {paths.map((path, index) => (
        <button
          key={path}
          type="button"
          role="option"
          aria-selected={index === activeIndex}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => onSelect(path)}
          className={cn(
            'flex w-full items-center gap-2 px-3 py-1 text-left',
            index === activeIndex ? 'bg-muted' : 'hover:bg-muted/60',
          )}
        >
          <FileText className="size-3 shrink-0 text-faint" aria-hidden="true" />
          <span className="truncate font-mono text-[10.5px] text-body">{path}</span>
        </button>
      ))}
    </div>
  );
}

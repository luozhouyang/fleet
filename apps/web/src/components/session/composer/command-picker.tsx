import type { SlashCommand } from '@/lib/session/composer-model';
import { cn } from '@/lib/utils/cn';

/** The `/` command palette shown above the composer. */
export function CommandPicker({
  commands,
  activeIndex,
  onSelect,
}: {
  commands: SlashCommand[];
  activeIndex: number;
  onSelect: (command: SlashCommand) => void;
}) {
  if (commands.length === 0) return null;
  return (
    <div
      role="listbox"
      aria-label="Composer commands"
      className="absolute inset-x-0 bottom-full z-20 mb-1.5 overflow-hidden rounded-lg border border-border bg-card shadow-floating"
    >
      {commands.map((command, index) => (
        <button
          key={command.name}
          type="button"
          role="option"
          aria-selected={index === activeIndex}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => onSelect(command)}
          className={cn(
            'flex w-full items-center gap-2 px-3 py-1.5 text-left',
            index === activeIndex ? 'bg-muted' : 'hover:bg-muted/60',
          )}
        >
          <span className="font-mono text-[11px] font-bold text-foreground">/{command.name}</span>
          <span className="truncate text-[10.5px] text-muted-foreground">
            {command.description}
          </span>
        </button>
      ))}
    </div>
  );
}

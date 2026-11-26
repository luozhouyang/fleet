import { Monitor, Moon, Sun } from 'lucide-react';
import { Tooltip } from '@/components/ui/tooltip';
import { useTheme } from '@/lib/use-theme';
import { cn } from '@/lib/utils/cn';
import type { ThemeMode } from '@/lib/theme';

const MODES: Array<{ mode: ThemeMode; label: string; icon: typeof Sun }> = [
  { mode: 'light', label: 'Light', icon: Sun },
  { mode: 'dark', label: 'Dark', icon: Moon },
  { mode: 'system', label: 'System', icon: Monitor },
];

/** Light / dark / system selector used in the top bar. */
export function ThemeToggle({ className }: { className?: string }) {
  const { mode, setMode } = useTheme();

  return (
    <div
      role="radiogroup"
      aria-label="Color theme"
      className={cn(
        'flex items-center gap-0.5 rounded-md border border-border bg-card p-0.5',
        className,
      )}
    >
      {MODES.map(({ mode: value, label, icon: Icon }) => (
        <Tooltip key={value} content={`${label} theme`}>
          <button
            type="button"
            role="radio"
            aria-checked={mode === value}
            aria-label={`${label} theme`}
            onClick={() => setMode(value)}
            className={cn(
              'flex size-6 items-center justify-center rounded-sm transition-colors',
              mode === value
                ? 'bg-muted text-foreground'
                : 'text-faint hover:bg-muted/60 hover:text-foreground',
            )}
          >
            <Icon className="size-3.5" aria-hidden="true" />
          </button>
        </Tooltip>
      ))}
    </div>
  );
}

import { SquareTerminal } from 'lucide-react';
import { EmptyState } from '@/components/ui/feedback';
import { useSessionStore } from '@/lib/session-store';
import { cn } from '@/lib/utils/cn';

/**
 * Agent-owned terminals.
 *
 * These are display-only: the agent owns the process and Fleet never proxies
 * input into it. Output is capped to its tail by the reducer.
 */
export function SessionTerminalsTab({ sessionId }: { sessionId: string }) {
  const terminals = useSessionStore((state) => state.sessions[sessionId]?.terminals);
  const entries = Object.entries(terminals ?? {});

  if (entries.length === 0) {
    return (
      <EmptyState
        title="No agent terminals"
        description="Terminal output the agent streams while it works appears here. These are display-only; Fleet does not attach to them."
        className="flex-1"
      />
    );
  }

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-5 pt-3">
      <p className="mb-2 rounded-md border border-border bg-muted/50 px-2 py-1.5 text-[10px] leading-4 text-muted-foreground">
        Agent-owned terminals. Display-only; output is not replayed after a reload.
      </p>
      <div className="flex flex-col gap-2">
        {entries.map(([id, terminal]) => (
          <section key={id} className="overflow-hidden rounded-md border border-code-border bg-code-bg">
            <header className="flex items-center gap-1.5 border-b border-code-border px-2 py-1">
              <SquareTerminal className="size-3 shrink-0 text-faint" aria-hidden="true" />
              <span className="min-w-0 flex-1 truncate font-mono text-[9.5px] text-muted-foreground">
                {terminal.command ?? id}
              </span>
              <span
                className={cn(
                  'shrink-0 font-mono text-[8.5px] uppercase',
                  terminal.done ? 'text-faint' : 'text-status-running',
                )}
              >
                {terminal.done ? 'exited' : 'running'}
              </span>
            </header>
            {terminal.cwd ? (
              <p className="truncate border-b border-code-border px-2 py-0.5 font-mono text-[8.5px] text-faint">
                {terminal.cwd}
              </p>
            ) : null}
            <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all p-2 font-mono text-[10px] leading-[15px] text-code-fg">
              {terminal.text || 'Waiting for output…'}
            </pre>
          </section>
        ))}
      </div>
    </div>
  );
}

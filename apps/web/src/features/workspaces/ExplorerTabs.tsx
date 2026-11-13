import { cn } from '@agent-fleet/ui';
import type { ExplorerTab } from './workspace-types';

export function ExplorerTabs({
  tabs,
  activeId,
  onSelect,
  onClose,
}: {
  tabs: ExplorerTab[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
}) {
  return (
    <div
      role="tablist"
      aria-label="Open files and diffs"
      className="flex shrink-0 items-stretch overflow-x-auto border-b border-slate-800/80 bg-slate-950/30"
    >
      {tabs.map((tab) => {
        const active = tab.id === activeId;
        return (
          <div
            key={tab.id}
            className={cn(
              'group flex min-w-0 max-w-72 items-center gap-1 border-r border-slate-800/70',
              active ? 'bg-slate-900/80' : 'hover:bg-slate-900/40',
            )}
          >
            <button
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => onSelect(tab.id)}
              title={tab.path}
              className={cn(
                'flex min-w-0 items-center gap-2 py-2 pl-3 pr-1 text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-teal-400/50',
                active ? 'text-slate-100' : 'text-slate-500',
              )}
            >
              {tab.kind === 'file' ? (
                <svg viewBox="0 0 16 16" fill="none" className="size-3.5 shrink-0" aria-hidden="true">
                  <path
                    d="M4 1.5h5L12.5 5v9.5H4V1.5Z"
                    stroke="currentColor"
                    strokeWidth="1.2"
                    strokeLinejoin="round"
                  />
                </svg>
              ) : (
                <svg viewBox="0 0 16 16" fill="none" className="size-3.5 shrink-0" aria-hidden="true">
                  <path
                    d="M2.5 4.5h4L8 6h5.5v7.5h-11v-9Z"
                    stroke="currentColor"
                    strokeWidth="1.2"
                    strokeLinejoin="round"
                  />
                  <path d="M5.5 9h5" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
                </svg>
              )}
              <span className="truncate">{tab.path}</span>
              {tab.kind === 'diff' ? (
                <span className="shrink-0 rounded bg-teal-500/10 px-1 py-0.5 text-[9px] uppercase text-teal-300">
                  diff
                </span>
              ) : null}
            </button>
            <button
              type="button"
              aria-label={`Close ${tab.path}`}
              onClick={() => onClose(tab.id)}
              className="mr-1 flex size-6 shrink-0 items-center justify-center rounded text-slate-600 opacity-0 transition-opacity hover:bg-slate-800 hover:text-slate-200 focus:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60 group-hover:opacity-100"
            >
              <svg viewBox="0 0 16 16" fill="none" className="size-3" aria-hidden="true">
                <path
                  d="m4 4 8 8m0-8-8 8"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                />
              </svg>
            </button>
          </div>
        );
      })}
    </div>
  );
}

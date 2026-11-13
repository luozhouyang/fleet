import { Link } from '@tanstack/react-router';
import { ConnectionIndicator } from './ConnectionIndicator';
import { TokenControl } from './TokenControl';

export function TopBar() {
  return (
    <header className="sticky top-0 z-20 flex h-16 items-center justify-between gap-4 border-b border-slate-800/80 bg-slate-950/75 px-4 backdrop-blur-md sm:px-6">
      <div className="flex min-w-0 items-center gap-3">
        <Link
          to="/machines"
          className="flex items-center gap-2 md:hidden"
          aria-label="Agent Fleet home"
        >
          <span className="flex size-7 items-center justify-center rounded-md border border-teal-400/25 bg-teal-500/10 text-[11px] font-bold text-teal-300">
            AF
          </span>
          <span className="text-sm font-semibold text-slate-100">Agent Fleet</span>
        </Link>
        <div className="hidden min-w-0 items-center gap-2 text-xs text-slate-500 md:flex">
          <span className="size-1.5 rounded-full bg-teal-400/80" />
          <span className="font-medium text-slate-300">Remote operations</span>
          <span className="text-slate-700">/</span>
          <span className="truncate">Machines, workspaces and coding agents</span>
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-2">
        <TokenControl />
        <ConnectionIndicator />
      </div>
    </header>
  );
}

import { Link } from '@tanstack/react-router';
import { FleetTree } from './FleetTree';
import { NavLinks } from './NavLinks';

export function Sidebar() {
  return (
    <aside className="fixed inset-y-0 left-0 z-30 hidden w-60 flex-col border-r border-slate-800/80 bg-slate-950/85 backdrop-blur md:flex">
      <Link
        to="/machines"
        className="flex h-16 items-center gap-3 border-b border-slate-800/80 px-5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-teal-400/60"
      >
        <span className="relative flex size-8 items-center justify-center rounded-lg border border-teal-400/25 bg-teal-500/10 text-sm font-bold text-teal-300">
          AF
          <span className="absolute -right-0.5 -top-0.5 size-2 rounded-full border-2 border-slate-950 bg-teal-400" />
        </span>
        <span className="min-w-0">
          <span className="block text-sm font-semibold tracking-tight text-slate-100">
            Agent Fleet
          </span>
          <span className="block text-[10px] uppercase tracking-[0.16em] text-slate-600">
            Operations
          </span>
        </span>
      </Link>

      <div className="flex-1 overflow-y-auto px-3 py-5">
        <div className="mb-2 px-3 text-[10px] font-semibold uppercase tracking-[0.18em] text-slate-600">
          Workspace
        </div>
        <NavLinks />
        <FleetTree />
      </div>

      <div className="border-t border-slate-800/80 p-4">
        <div className="rounded-lg border border-slate-800/80 bg-slate-900/50 p-3">
          <div className="flex items-center justify-between text-[11px] text-slate-500">
            <span>Protocol</span>
            <span className="font-mono text-slate-400">v1</span>
          </div>
          <p className="mt-1.5 text-[11px] leading-4 text-slate-600">
            Remote coding agents on your own machines.
          </p>
        </div>
      </div>
    </aside>
  );
}

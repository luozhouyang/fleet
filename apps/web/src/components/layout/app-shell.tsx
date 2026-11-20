import { Outlet } from '@tanstack/react-router';
import { TokenExpiryBanner } from '@/features/users/TokenExpiryBanner';
import { dismissLag, useLagNotice } from '@/lib/events/lag-store';
import { TopBar } from './top-bar';

export function AppShell() {
  const lag = useLagNotice();

  return (
    <div className="flex h-dvh min-h-0 flex-col overflow-hidden bg-background">
      <TopBar />
      <TokenExpiryBanner />
      {lag ? (
        <div
          role="status"
          className="flex shrink-0 items-center justify-between gap-3 border-b border-border bg-warning-soft px-5 py-2 text-[11px] text-warning"
        >
          <span>
            Live updates were interrupted
            {lag.skipped > 0 ? ` and ${lag.skipped} events were skipped` : ''}. Open views have been
            refreshed.
          </span>
          <button
            type="button"
            onClick={dismissLag}
            className="shrink-0 font-bold text-warning hover:underline"
          >
            Dismiss
          </button>
        </div>
      ) : null}
      <main className="min-h-0 flex-1 overflow-hidden">
        <Outlet />
      </main>
    </div>
  );
}

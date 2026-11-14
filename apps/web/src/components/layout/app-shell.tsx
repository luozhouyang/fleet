import { Outlet } from '@tanstack/react-router';
import { TopBar } from './top-bar';

export function AppShell() {
  return (
    <div className="flex h-dvh min-h-0 flex-col overflow-hidden bg-background">
      <TopBar />
      <main className="min-h-0 flex-1 overflow-hidden">
        <Outlet />
      </main>
    </div>
  );
}

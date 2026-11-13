import { Outlet } from '@tanstack/react-router';
import { NavLinks } from './NavLinks';
import { Sidebar } from './Sidebar';
import { TopBar } from './TopBar';

export function AppShell() {
  return (
    <div className="min-h-screen text-slate-200">
      <Sidebar />
      <div className="min-h-screen md:pl-60">
        <TopBar />
        <div className="border-b border-slate-800/60 md:hidden">
          <NavLinks variant="mobile" />
        </div>
        <main className="mx-auto w-full max-w-[1800px] px-4 py-5 sm:px-6 sm:py-6">
          <Outlet />
        </main>
      </div>
    </div>
  );
}

import { Link, useRouterState } from '@tanstack/react-router';
import { cn } from '@agent-fleet/ui';

const links = [
  {
    to: '/machines' as const,
    label: 'Machines',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" className="size-4" aria-hidden="true">
        <rect x="3" y="4" width="18" height="7" rx="2" stroke="currentColor" strokeWidth="1.5" />
        <rect x="3" y="13" width="18" height="7" rx="2" stroke="currentColor" strokeWidth="1.5" />
        <path d="M7 7.5h.01M7 16.5h.01M11 7.5h5M11 16.5h5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      </svg>
    ),
  },
  {
    to: '/sessions' as const,
    label: 'Sessions',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" className="size-4" aria-hidden="true">
        <path
          d="M5 6.5A2.5 2.5 0 0 1 7.5 4h9A2.5 2.5 0 0 1 19 6.5v7a2.5 2.5 0 0 1-2.5 2.5H10l-5 4V6.5Z"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinejoin="round"
        />
        <path d="M9 8.5h6M9 12h3.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      </svg>
    ),
  },
];

export function NavLinks({ variant = 'sidebar' }: { variant?: 'sidebar' | 'mobile' }) {
  const pathname = useRouterState({ select: (state) => state.location.pathname });

  return (
    <nav
      aria-label="Primary"
      className={variant === 'sidebar' ? 'space-y-1' : 'flex gap-1 overflow-x-auto px-3 py-2'}
    >
      {links.map((link) => {
        const active = pathname === link.to || pathname.startsWith(`${link.to}/`);
        return (
          <Link
            key={link.to}
            to={link.to}
            aria-current={active ? 'page' : undefined}
            className={cn(
              'flex items-center gap-2.5 rounded-md px-3 py-2 text-sm font-medium transition-colors',
              variant === 'mobile' && 'shrink-0 py-1.5',
              active
                ? 'bg-teal-500/10 text-teal-300 ring-1 ring-inset ring-teal-500/20'
                : 'text-slate-400 hover:bg-slate-900 hover:text-slate-200',
            )}
          >
            {link.icon}
            {link.label}
          </Link>
        );
      })}
    </nav>
  );
}

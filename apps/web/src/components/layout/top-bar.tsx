import { MachineStatus } from '@agent-fleet/protocol-types';
import { Link, useNavigate } from '@tanstack/react-router';
import { Settings } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useSessionList, useMachines } from '@/lib/query/hooks';
import { useMe } from '@/features/users/hooks';
import { roleIsAdmin } from '@/features/users/user-utils';
import { OrganizationSwitcher } from '@/features/organizations/OrganizationSwitcher';
import { useEventConnectionState } from '@/lib/events/useEventConnection';
import { sessionIsActive } from '@/lib/status';
import { Kbd } from '@/components/ui/kbd';
import { StatusDot } from '@/components/ui/status-dot';
import { ThemeToggle } from '@/components/ui/theme-toggle';
import { CommandPalette } from '@/components/layout/command-palette';
import { SettingsDialog } from '@/components/dialogs/settings-dialog';
import { NewSessionDialog } from '@/components/dialogs/new-session-dialog';
import type { StatusTone } from '@/lib/status';

const NAV = [
  { to: '/sessions' as const, label: 'Sessions' },
  { to: '/workspaces' as const, label: 'Workspaces' },
  { to: '/machines' as const, label: 'Machines' },
];

const USERS_NAV = { to: '/users' as const, label: 'Users' };

function liveTone(connection: string): StatusTone {
  if (connection === 'connected') return 'running';
  if (connection === 'reconnecting' || connection === 'connecting') return 'waiting';
  if (connection === 'failed') return 'failed';
  return 'offline';
}

function liveLabel(connection: string): string {
  if (connection === 'connected') return 'Live';
  if (connection === 'reconnecting' || connection === 'connecting') return 'Connecting';
  return 'Offline';
}

function BrandMark() {
  return (
    <span className="flex size-[18px] flex-col justify-center gap-[2px]" aria-hidden="true">
      <span className="h-[3px] w-[18px] rounded-full bg-foreground" />
      <span className="h-[3px] w-[12px] rounded-full bg-primary" />
      <span className="h-[3px] w-[6px] rounded-full bg-foreground" />
    </span>
  );
}

export function TopBar() {
  const navigate = useNavigate();
  const sessionsQuery = useSessionList();
  const machinesQuery = useMachines();
  const meQuery = useMe();
  const connection = useEventConnectionState();
  const [commandOpen, setCommandOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [newSessionOpen, setNewSessionOpen] = useState(false);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setCommandOpen((current) => !current);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const running = (sessionsQuery.data ?? []).filter((session) =>
    sessionIsActive(session.status),
  ).length;
  const online = (machinesQuery.data ?? []).filter(
    (machine) =>
      machine.status === MachineStatus.Online || machine.status === MachineStatus.Degraded,
  ).length;
  const machineCount = machinesQuery.data?.length ?? 0;
  const nav = roleIsAdmin(meQuery.data?.role) ? [...NAV, USERS_NAV] : NAV;

  return (
    <>
      <header className="flex h-14 shrink-0 items-center justify-between border-b border-border bg-background px-5">
        <div className="flex h-14 items-center gap-6">
          <Link to="/sessions" className="flex items-center gap-3">
            <BrandMark />
            <span className="font-display text-[12px] font-bold tracking-[0.14em] text-foreground">
              AGENT FLEET
            </span>
          </Link>
          <span className="h-[18px] w-px bg-border-strong" />
          <OrganizationSwitcher />
          <span className="h-[18px] w-px bg-border-strong" />
          <nav className="flex h-14 items-end gap-6">
            {nav.map((item) => (
              <Link
                key={item.to}
                to={item.to}
                className="relative flex h-14 items-end pb-[13px] text-[12px] font-medium text-muted-foreground transition-colors after:absolute after:inset-x-0 after:bottom-0 after:h-[2px] after:rounded-full after:bg-transparent hover:text-foreground data-[status=active]:font-bold data-[status=active]:text-foreground data-[status=active]:after:bg-primary"
              >
                {item.label}
              </Link>
            ))}
          </nav>
        </div>

        <div className="flex items-center gap-3.5">
          <span className="flex items-center gap-1.5">
            <StatusDot
              tone={liveTone(connection)}
              size="sm"
              pulse={connection === 'connected'}
              label={liveLabel(connection)}
            />
            <span className="font-mono text-[9.5px] text-muted-foreground">
              · {running} running / {online || machineCount} machines
            </span>
          </span>
          <button
            type="button"
            onClick={() => setCommandOpen(true)}
            aria-label="Open command palette"
            className="rounded focus-visible:ring-2 focus-visible:ring-ring/30"
          >
            <Kbd>⌘K</Kbd>
          </button>
          <ThemeToggle className="hidden sm:flex" />
          <button
            type="button"
            onClick={() => setSettingsOpen(true)}
            aria-label="Connection settings"
            className="flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          >
            <Settings className="size-3.5" />
          </button>
          <span className="flex size-[26px] items-center justify-center rounded-full bg-primary-soft text-[9.5px] font-bold text-primary">
            LZ
          </span>
        </div>
      </header>

      <CommandPalette
        open={commandOpen}
        onOpenChange={setCommandOpen}
        onNewSession={() => setNewSessionOpen(true)}
      />
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
      <NewSessionDialog
        open={newSessionOpen}
        onOpenChange={setNewSessionOpen}
        machines={machinesQuery.data ?? []}
        onCreated={(session) => {
          void navigate({
            to: '/sessions/$sessionId',
            params: { sessionId: session.id },
          });
        }}
      />
    </>
  );
}

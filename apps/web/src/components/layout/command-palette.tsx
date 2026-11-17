import { FolderGit2, Monitor, Plus, Search, Terminal } from 'lucide-react';
import { useNavigate } from '@tanstack/react-router';
import { useEffect, useMemo, useState } from 'react';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { Kbd } from '@/components/ui/kbd';
import { useMachines, useSessionList, useWorkspaceIndex } from '@/lib/query/hooks';
import { shortId } from '@/lib/utils/format';
import { cn } from '@/lib/utils/cn';
import { machineDisplayName } from '@/lib/utils/machine';

function normalize(value: string | undefined | null): string {
  return (value ?? '').toLowerCase();
}

function commandItems(root: ParentNode | null): HTMLButtonElement[] {
  return Array.from(root?.querySelectorAll<HTMLButtonElement>('[data-command-item]') ?? []);
}

function moveCommandFocus(
  root: ParentNode | null,
  offset: 1 | -1,
  current?: EventTarget | null,
): void {
  const items = commandItems(root);
  if (items.length === 0) return;
  const index = current instanceof HTMLElement ? items.indexOf(current as HTMLButtonElement) : -1;
  items[(index + offset + items.length) % items.length]?.focus();
}

function commandItemKeyDown(event: React.KeyboardEvent<HTMLButtonElement>): void {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
  event.preventDefault();
  moveCommandFocus(
    event.currentTarget.closest('[data-command-list]'),
    event.key === 'ArrowDown' ? 1 : -1,
    event.currentTarget,
  );
}

export function CommandPalette({
  open,
  onOpenChange,
  onNewSession,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onNewSession: () => void;
}) {
  const navigate = useNavigate();
  const sessions = useSessionList().data;
  const machines = useMachines().data;
  const { locations } = useWorkspaceIndex();
  const [query, setQuery] = useState('');

  useEffect(() => {
    if (open) setQuery('');
  }, [open]);

  const q = normalize(query.trim());

  const sessionItems = useMemo(
    () =>
      (sessions ?? [])
        .filter((session) => {
          if (!q) return true;
          return (
            normalize(session.title).includes(q) ||
            normalize(session.agent_type).includes(q) ||
            normalize(session.provider).includes(q) ||
            normalize(session.id).includes(q)
          );
        })
        .slice(0, 6),
    [sessions, q],
  );

  const workspaceItems = useMemo(
    () =>
      locations
        .filter(({ machine, workspace }) => {
          if (!q) return true;
          return (
            normalize(workspace.name).includes(q) ||
            normalize(machineDisplayName(machine)).includes(q) ||
            normalize(workspace.root_path).includes(q)
          );
        })
        .slice(0, 5),
    [locations, q],
  );

  const machineItems = useMemo(
    () =>
      (machines ?? [])
        .filter((machine) => {
          if (!q) return true;
          return (
            normalize(machineDisplayName(machine)).includes(q) ||
            normalize(machine.hostname).includes(q)
          );
        })
        .slice(0, 4),
    [machines, q],
  );

  const close = () => onOpenChange(false);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[540px] overflow-hidden p-0">
        <DialogTitle className="sr-only">Command palette</DialogTitle>
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <Search className="size-4 text-faint" />
          <input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
              event.preventDefault();
              moveCommandFocus(
                document.querySelector('[data-command-list]'),
                event.key === 'ArrowDown' ? 1 : -1,
              );
            }}
            placeholder="Jump to a session, workspace or machine…"
            className="h-6 flex-1 rounded-sm bg-transparent text-[12.5px] text-foreground placeholder:text-faint"
            aria-label="Command palette search"
          />
          <Kbd>ESC</Kbd>
        </div>
        <div data-command-list className="max-h-[400px] overflow-y-auto p-2">
          {!q ? (
            <button
              type="button"
              data-command-item
              onClick={() => {
                close();
                onNewSession();
              }}
              onKeyDown={commandItemKeyDown}
              className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-[12px] text-body hover:bg-muted focus-visible:bg-muted"
            >
              <Plus className="size-3.5 text-primary" />
              New session
              <span className="ml-auto font-mono text-[9px] text-faint">action</span>
            </button>
          ) : null}

          {sessionItems.length ? (
            <Section title="Sessions">
              {sessionItems.map((session) => (
                <Item
                  key={session.id}
                  icon={<Terminal className="size-3.5 text-primary" />}
                  label={session.title || `Session ${shortId(session.id, 8)}`}
                  meta={`${session.provider} · ${session.status}`}
                  onSelect={() => {
                    close();
                    void navigate({
                      to: '/sessions/$sessionId',
                      params: { sessionId: session.id },
                    });
                  }}
                />
              ))}
            </Section>
          ) : null}

          {workspaceItems.length ? (
            <Section title="Workspaces">
              {workspaceItems.map(({ machine, workspace }) => (
                <Item
                  key={workspace.id}
                  icon={<FolderGit2 className="size-3.5 text-muted-foreground" />}
                  label={workspace.name}
                  meta={`${machineDisplayName(machine)} · ${workspace.root_path}`}
                  onSelect={() => {
                    close();
                    void navigate({
                      to: '/workspaces/$workspaceId',
                      params: { workspaceId: workspace.id },
                    });
                  }}
                />
              ))}
            </Section>
          ) : null}

          {machineItems.length ? (
            <Section title="Machines">
              {machineItems.map((machine) => (
                <Item
                  key={machine.id}
                  icon={<Monitor className="size-3.5 text-muted-foreground" />}
                  label={machineDisplayName(machine)}
                  meta={`${machine.os} · ${machine.status}`}
                  onSelect={() => {
                    close();
                    void navigate({
                      to: '/machines/$machineId',
                      params: { machineId: machine.id },
                    });
                  }}
                />
              ))}
            </Section>
          ) : null}

          {!sessionItems.length && !workspaceItems.length && !machineItems.length ? (
            <p className="px-3 py-8 text-center text-[12px] text-muted-foreground">
              Nothing matches “{query}”.
            </p>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mt-1">
      <p className="px-2.5 py-1.5 font-mono text-[8.5px] uppercase tracking-[0.08em] text-faint">
        {title}
      </p>
      <div className="flex flex-col gap-0.5">{children}</div>
    </div>
  );
}

function Item({
  icon,
  label,
  meta,
  onSelect,
}: {
  icon: React.ReactNode;
  label: string;
  meta: string;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      data-command-item
      onClick={onSelect}
      onKeyDown={commandItemKeyDown}
      className={cn(
        'flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left transition-colors hover:bg-muted focus-visible:bg-muted',
      )}
    >
      {icon}
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[12px] font-bold text-foreground">{label}</span>
        <span className="block truncate font-mono text-[9px] text-faint">{meta}</span>
      </span>
    </button>
  );
}

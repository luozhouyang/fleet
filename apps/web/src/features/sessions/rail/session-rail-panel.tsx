import { MachineStatus, type AgentSession } from '@agent-fleet/protocol-types';
import { useNavigate } from '@tanstack/react-router';
import { ChevronDown, Plus, Search, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ConfirmDialog } from '@/components/dialogs/confirm-dialog';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { Input } from '@/components/ui/input';
import { Kbd } from '@/components/ui/kbd';
import { Menu, MenuContent, MenuItem, MenuTrigger } from '@/components/ui/menu';
import { apiClient } from '@/lib/api/client';
import { useEventConnectionState } from '@/lib/events/useEventConnection';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useMachines, useSessionFacets, useSessionRail } from '@/lib/query/hooks';
import {
  useMarkSessionUnread,
  useMarkSessionsRead,
  useUpdateSessionTitle,
  useUpdateSessionsUi,
} from '@/lib/query/mutations';
import { sessionTitle } from '@/components/session/session-utils';
import { cn } from '@/lib/utils/cn';
import { getErrorMessage } from '@/lib/utils/format';
import { machineDisplayName } from '@/lib/utils/machine';
import {
  GROUP_MODES,
  RAIL_FILTERS,
  buildRailRows,
  groupSessions,
  hasAttention,
  matchesFilter,
  matchesLabels,
  matchesSearch,
  type RailFilter,
} from '../rail-model';
import { useRailPreferences } from '../rail-preferences';
import { SessionRail } from './session-rail';
import type { SessionRowAction } from './session-row-menu';

/**
 * The session rail: header, filters, search, groups and the virtualized list.
 *
 * Data comes from the paged rail query; the legacy list endpoint is untouched
 * for the command palette and top-bar counter.
 */
export function SessionRailPanel({
  selectedId,
  onNewSession,
  onCloseMobile,
}: {
  selectedId: string;
  onNewSession: () => void;
  onCloseMobile: () => void;
}) {
  const navigate = useNavigate();
  const machinesQuery = useMachines();
  const connection = useEventConnectionState();
  const { preferences, setGroupMode, toggleGroup, setSmartSort } = useRailPreferences();

  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState<RailFilter>('all');
  const [labelFilter, setLabelFilter] = useState<string[]>([]);
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [multiSelected, setMultiSelected] = useState<Set<string>>(new Set());
  const [renaming, setRenaming] = useState<{ id: string; value: string } | null>(null);
  const [deleting, setDeleting] = useState<AgentSession | null>(null);
  const [closing, setClosing] = useState<AgentSession | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const railQuery = useSessionRail({
    includeClosed: true,
    includeArchived: true,
    sort: 'recent',
    limit: 1000,
  });
  const facetsQuery = useSessionFacets();
  const machines = useMemo(() => machinesQuery.data ?? [], [machinesQuery.data]);

  const updateUi = useUpdateSessionsUi();
  const markRead = useMarkSessionsRead();
  const markUnread = useMarkSessionUnread();
  const renameTitle = useUpdateSessionTitle();
  const queryClient = useQueryClient();
  const deleteSession = useMutation({
    mutationFn: (id: string) => apiClient.deleteSession(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['sessions', 'rail'] });
      void queryClient.invalidateQueries({ queryKey: ['sessions', 'facets'] });
    },
  });

  const sessions = useMemo(() => railQuery.data?.items ?? [], [railQuery.data]);

  const machineNames = useMemo(
    () => new Map(machines.map((machine) => [machine.id, machineDisplayName(machine)] as const)),
    [machines],
  );
  const workspaceNames = useMemo(
    () =>
      new Map(
        machines.flatMap((machine) =>
          (machine.workspaces ?? []).map((workspace) => [workspace.id, workspace.name] as const),
        ),
      ),
    [machines],
  );
  const offlineMachines = useMemo(
    () =>
      new Set(
        machines
          .filter(
            (machine) =>
              machine.status !== MachineStatus.Online && machine.status !== MachineStatus.Degraded,
          )
          .map((machine) => machine.id),
      ),
    [machines],
  );
  const allLabels = useMemo(() => {
    const labels = new Set<string>();
    for (const session of sessions) {
      for (const label of session.ui?.labels ?? []) labels.add(label);
    }
    return [...labels].sort();
  }, [sessions]);

  const visible = useMemo(() => {
    const term = search.trim();
    return sessions.filter((session) => {
      if (!matchesFilter(session, filter)) return false;
      if (!matchesLabels(session, labelFilter)) return false;
      return matchesSearch(session, term, {
        machine: machineNames.get(session.machine_id),
        workspace: session.workspace_id ? workspaceNames.get(session.workspace_id) : undefined,
      });
    });
  }, [sessions, filter, labelFilter, search, machineNames, workspaceNames]);

  const namesBySession = useMemo(
    () =>
      new Map(
        sessions.map((session) => [
          session.id,
          {
            machine: machineNames.get(session.machine_id),
            workspace: session.workspace_id ? workspaceNames.get(session.workspace_id) : undefined,
          },
        ]),
      ),
    [sessions, machineNames, workspaceNames],
  );

  const groups = useMemo(
    () => groupSessions(visible, preferences.groupMode, namesBySession),
    [visible, preferences.groupMode, namesBySession],
  );
  const rows = useMemo(
    () => buildRailRows(groups, new Set(preferences.collapsedGroups), preferences.groupMode),
    [groups, preferences.collapsedGroups, preferences.groupMode],
  );

  // Attention: switch to the smart order the first time a session needs you
  // while the user is looking, and keep it until they turn it off.
  const needsAttention = hasAttention(sessions);
  useEffect(() => {
    if (!needsAttention || preferences.smartSort) return;
    setSmartSort(true);
  }, [needsAttention, preferences.smartSort, setSmartSort]);

  const sessionRows = rows.filter((row) => row.kind === 'session');

  const moveFocus = useCallback(
    (delta: number) => {
      if (sessionRows.length === 0) return;
      const currentIndex = sessionRows.findIndex(
        (row) => row.kind === 'session' && row.session.id === focusedId,
      );
      const nextIndex =
        currentIndex === -1
          ? delta > 0
            ? 0
            : sessionRows.length - 1
          : Math.min(Math.max(currentIndex + delta, 0), sessionRows.length - 1);
      const row = sessionRows[nextIndex];
      if (row.kind === 'session') {
        setFocusedId(row.session.id);
        // Keep the link's href in sync so Enter opens the focused session.
      }
    },
    [sessionRows, focusedId],
  );

  const activate = useCallback(
    (session: AgentSession) => {
      if (session.ui?.unread) {
        markRead.mutate({ ids: [session.id], watermark: session.updated_at });
      }
      void navigate({ to: '/sessions/$sessionId', params: { sessionId: session.id } });
    },
    [markRead, navigate],
  );

  const runAction = useCallback(
    (session: AgentSession, action: SessionRowAction) => {
      setActionError(null);
      switch (action) {
        case 'pin':
        case 'unpin':
          updateUi.mutate(
            { ids: [session.id], pinned: action === 'pin' },
            { onError: (error) => setActionError(getErrorMessage(error)) },
          );
          break;
        case 'archive':
        case 'unarchive':
          updateUi.mutate(
            { ids: [session.id], archived: action === 'archive' },
            { onError: (error) => setActionError(getErrorMessage(error)) },
          );
          break;
        case 'mark-read':
          markRead.mutate({ ids: [session.id], watermark: session.updated_at });
          break;
        case 'mark-unread':
          markUnread.mutate(session.id, {
            onError: (error) => setActionError(getErrorMessage(error)),
          });
          break;
        case 'rename':
          setRenaming({ id: session.id, value: sessionTitle(session) });
          break;
        case 'close':
          setClosing(session);
          break;
        case 'delete':
          setDeleting(session);
          break;
      }
    },
    [markRead, markUnread, updateUi],
  );

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) {
        if (event.key === 'Escape') (event.target as HTMLElement).blur();
        return;
      }
      switch (event.key) {
        case 'ArrowDown':
          event.preventDefault();
          moveFocus(1);
          break;
        case 'ArrowUp':
          event.preventDefault();
          moveFocus(-1);
          break;
        case 'Home':
          event.preventDefault();
          if (sessionRows[0]?.kind === 'session') setFocusedId(sessionRows[0].session.id);
          break;
        case 'End': {
          event.preventDefault();
          const last = sessionRows[sessionRows.length - 1];
          if (last?.kind === 'session') setFocusedId(last.session.id);
          break;
        }
        case 'Enter': {
          if (!focusedId) break;
          event.preventDefault();
          const target = sessions.find((session) => session.id === focusedId);
          if (target) activate(target);
          break;
        }
        case '/':
          event.preventDefault();
          searchRef.current?.focus();
          break;
        case 'p':
        case 'u':
        case 'e': {
          if (!focusedId || event.metaKey || event.ctrlKey) break;
          const target = sessions.find((session) => session.id === focusedId);
          if (!target) break;
          event.preventDefault();
          if (event.key === 'p') runAction(target, target.ui?.pinned ? 'unpin' : 'pin');
          if (event.key === 'u') runAction(target, target.ui?.unread ? 'mark-read' : 'mark-unread');
          if (event.key === 'e') runAction(target, target.ui?.archived ? 'unarchive' : 'archive');
          break;
        }
        default:
          break;
      }
    },
    [activate, focusedId, moveFocus, runAction, sessionRows, sessions],
  );

  const connectionLabel =
    connection === 'connected'
      ? 'Live'
      : connection === 'connecting'
        ? 'Connecting'
        : connection === 'reconnecting'
          ? 'Reconnecting'
          : connection === 'failed'
            ? 'Offline'
            : 'Offline';

  const facetCount = (id: RailFilter): number => {
    const facets = facetsQuery.data;
    if (!facets) {
      return sessions.filter((session) => matchesFilter(session, id)).length;
    }
    switch (id) {
      case 'all':
        return facets.all;
      case 'running':
        return facets.running;
      case 'waiting':
        return facets.waiting;
      case 'failed':
        return facets.failed;
      case 'closed':
        return facets.closed;
      case 'archived':
        return facets.archived;
    }
  };

  // Toggle multi-select with Cmd/Ctrl+click on a row.
  const onRowActivate = useCallback(
    (session: AgentSession, event: React.MouseEvent) => {
      if (event.metaKey || event.ctrlKey) {
        event.preventDefault();
        setMultiSelected((current) => {
          const next = new Set(current);
          if (next.has(session.id)) next.delete(session.id);
          else next.add(session.id);
          return next;
        });
        return;
      }
      if (multiSelected.size > 0) setMultiSelected(new Set());
      activate(session);
    },
    [activate, multiSelected.size],
  );

  return (
    <div className="flex h-full min-h-0 flex-col" onKeyDown={onKeyDown}>
      <div className="shrink-0 px-3 pt-3">
        <div className="flex items-center justify-between gap-2">
          <div className="flex min-w-0 items-baseline gap-2">
            <h2 className="font-display text-[18px] font-bold leading-6 tracking-[-0.01em] text-foreground">
              Sessions
            </h2>
            <span className="font-mono text-[9px] uppercase tracking-[0.1em] text-faint">
              {sessions.filter((session) => session.status !== 'terminated').length} active
            </span>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <Button
              variant="ghost"
              size="icon-sm"
              className="md:hidden"
              aria-label="Close session list"
              onClick={onCloseMobile}
            >
              <X className="size-4" />
            </Button>
            <Button
              variant="primary"
              size="icon-sm"
              aria-label="New session"
              title="New session"
              onClick={onNewSession}
            >
              <Plus className="size-3.5" />
            </Button>
          </div>
        </div>

        <label className="mt-2.5 flex h-8 items-center gap-2 rounded-md border border-border bg-card px-2 transition-[border-color,box-shadow] focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/20">
          <Search className="size-3.5 shrink-0 text-faint" aria-hidden="true" />
          <Input
            ref={searchRef}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            aria-label="Search sessions"
            placeholder="Search sessions…"
            className="h-full border-0 bg-transparent p-0 text-[11.5px] shadow-none focus-visible:ring-0"
          />
          {search ? (
            <button
              type="button"
              aria-label="Clear search"
              onClick={() => setSearch('')}
              className="shrink-0 text-faint hover:text-foreground"
            >
              <X className="size-3" />
            </button>
          ) : (
            <Kbd className="shrink-0">/</Kbd>
          )}
        </label>

        <div className="mt-2 flex flex-wrap items-center gap-1">
          {RAIL_FILTERS.map((chip) => (
            <button
              key={chip.id}
              type="button"
              onClick={() => setFilter(chip.id)}
              aria-pressed={filter === chip.id}
              className={cn(
                'flex h-[22px] items-center gap-1 rounded-full border px-2 font-mono text-[9px] font-bold uppercase tracking-[0.06em] transition-colors',
                filter === chip.id
                  ? 'border-primary bg-primary-soft text-primary'
                  : 'border-border text-faint hover:border-border-strong hover:text-foreground',
              )}
            >
              {chip.label}
              <span className="font-normal">{facetCount(chip.id)}</span>
            </button>
          ))}
        </div>

        {allLabels.length > 0 ? (
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            {allLabels.slice(0, 8).map((label) => {
              const active = labelFilter.includes(label);
              return (
                <button
                  key={label}
                  type="button"
                  onClick={() =>
                    setLabelFilter((current) =>
                      active ? current.filter((item) => item !== label) : [...current, label],
                    )
                  }
                  aria-pressed={active}
                  className={cn(
                    'flex h-[20px] items-center rounded-full border px-1.5 font-mono text-[9px] transition-colors',
                    active
                      ? 'border-status-attention/50 bg-primary-soft text-primary'
                      : 'border-border text-faint hover:text-foreground',
                  )}
                >
                  #{label}
                </button>
              );
            })}
          </div>
        ) : null}

        <div className="mt-2 flex items-center justify-between gap-2 pb-1.5">
          <Menu>
            <MenuTrigger
              render={
                <button
                  type="button"
                  className="flex h-6 items-center gap-1 rounded-md px-1 font-mono text-[9px] uppercase tracking-[0.08em] text-faint transition-colors hover:bg-muted hover:text-foreground"
                >
                  Group: {GROUP_MODES.find((mode) => mode.id === preferences.groupMode)?.label}
                  <ChevronDown className="size-3" />
                </button>
              }
            />
            <MenuContent align="start">
              {GROUP_MODES.map((mode) => (
                <MenuItem
                  key={mode.id}
                  onClick={() => setGroupMode(mode.id)}
                  className={preferences.groupMode === mode.id ? 'font-bold text-foreground' : ''}
                >
                  {mode.label}
                </MenuItem>
              ))}
            </MenuContent>
          </Menu>
          <button
            type="button"
            onClick={() => setSmartSort(!preferences.smartSort)}
            aria-pressed={preferences.smartSort}
            title="Order sessions that need you first"
            className={cn(
              'flex h-6 items-center rounded-md px-1 font-mono text-[9px] uppercase tracking-[0.08em] transition-colors',
              preferences.smartSort
                ? 'text-primary hover:bg-primary-soft'
                : 'text-faint hover:bg-muted hover:text-foreground',
            )}
          >
            Smart
          </button>
        </div>
      </div>

      <div className="mx-3 shrink-0 border-t border-border" />

      {railQuery.isPending ? (
        <LoadingState label="Loading sessions…" />
      ) : railQuery.isError ? (
        <ErrorState
          title="Could not load sessions"
          message={getErrorMessage(railQuery.error)}
          onRetry={() => void railQuery.refetch()}
        />
      ) : rows.length === 0 ? (
        <EmptyState
          title={search || filter !== 'all' ? 'No matching sessions' : 'No sessions yet'}
          description={
            search || filter !== 'all'
              ? 'Adjust the filter or search term.'
              : 'Start a session to run an agent on a machine.'
          }
          action={
            <Button variant="primary" onClick={onNewSession}>
              New session
            </Button>
          }
        />
      ) : (
        <SessionRail
          rows={rows}
          selectedId={selectedId}
          focusedId={focusedId}
          groupMode={preferences.groupMode}
          machineNames={machineNames}
          workspaceNames={workspaceNames}
          offlineMachines={offlineMachines}
          multiSelected={multiSelected}
          listLabel="Sessions"
          onFocusRow={setFocusedId}
          onToggleGroup={toggleGroup}
          onAction={runAction}
          onActivate={onRowActivate}
        />
      )}

      {multiSelected.size > 0 ? (
        <div className="mx-2 mb-2 flex shrink-0 items-center gap-1.5 rounded-lg border border-primary/40 bg-primary-soft px-2 py-1.5">
          <span className="font-mono text-[10px] text-primary">{multiSelected.size} selected</span>
          <span className="flex-1" />
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              const ids = [...multiSelected].filter((id) => {
                const session = sessions.find((item) => item.id === id);
                return session && !session.ui?.pinned;
              });
              if (ids.length) updateUi.mutate({ ids, pinned: true });
            }}
          >
            Pin
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              const ids = [...multiSelected];
              if (ids.length) updateUi.mutate({ ids, archived: true });
              setMultiSelected(new Set());
            }}
          >
            Archive
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setMultiSelected(new Set())}>
            Clear
          </Button>
        </div>
      ) : null}

      <div className="mx-3 mb-2 flex shrink-0 items-center justify-between border-t border-border pt-2 font-mono text-[9px] text-faint">
        <span className="flex items-center gap-1.5">
          <span
            aria-hidden="true"
            className={cn(
              'size-1.5 rounded-full',
              connection === 'connected' ? 'bg-success' : 'bg-warning',
            )}
          />
          {connectionLabel} · {machines.length} machines ·{' '}
          {machines.filter((machine) => machine.status === MachineStatus.Online).length} online
        </span>
        {actionError ? <span className="text-destructive">{actionError}</span> : null}
      </div>

      {renaming ? (
        <RenameDialog
          value={renaming.value}
          onChange={(value) => setRenaming({ id: renaming.id, value })}
          onCancel={() => setRenaming(null)}
          onSubmit={() => {
            renameTitle.mutate(
              { sessionId: renaming.id, title: renaming.value.trim() || null },
              { onSettled: () => setRenaming(null) },
            );
          }}
        />
      ) : null}

      <ConfirmDialog
        open={Boolean(deleting)}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title="Delete session"
        description={
          deleting
            ? `Delete “${sessionTitle(deleting)}” and its recorded history? This cannot be undone.`
            : ''
        }
        confirmLabel="Delete"
        danger
        onConfirm={() => {
          if (!deleting) return;
          const id = deleting.id;
          deleteSession.mutate(id, {
            onSettled: () => {
              setDeleting(null);
              if (selectedId === id) void navigate({ to: '/sessions' });
            },
          });
        }}
      />

      <ConfirmDialog
        open={Boolean(closing)}
        onOpenChange={(open) => {
          if (!open) setClosing(null);
        }}
        title="Close session"
        description={
          closing
            ? `Close “${sessionTitle(closing)}”? Its history is kept and it can no longer be resumed.`
            : ''
        }
        confirmLabel="Close"
        onConfirm={() => {
          if (!closing) return;
          const id = closing.id;
          void apiClient
            .closeSession(id)
            .catch((error) => setActionError(getErrorMessage(error)))
            .finally(() => setClosing(null));
        }}
      />
    </div>
  );
}

/** Inline rename input rendered as a small dialog-free overlay. */
function RenameDialog({
  value,
  onChange,
  onCancel,
  onSubmit,
}: {
  value: string;
  onChange: (value: string) => void;
  onCancel: () => void;
  onSubmit: () => void;
}) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-overlay/30 p-4">
      <form
        role="dialog"
        aria-label="Rename session"
        className="w-full max-w-[360px] animate-rise rounded-xl border border-border bg-card p-4 shadow-modal"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
      >
        <label className="text-[11px] font-bold text-muted-foreground" htmlFor="session-rename">
          Session title
        </label>
        <Input
          id="session-rename"
          autoFocus
          value={value}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') onCancel();
          }}
          className="mt-1.5"
        />
        <div className="mt-3 flex justify-end gap-2">
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" variant="primary">
            Save
          </Button>
        </div>
      </form>
    </div>
  );
}

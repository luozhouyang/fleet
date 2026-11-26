import { useVirtualizer } from '@tanstack/react-virtual';
import type { AgentSession } from '@agent-fleet/protocol-types';
import { useEffect, useRef } from 'react';
import { cn } from '@/lib/utils/cn';
import type { RailGroupMode, RailRow } from '../rail-model';
import { SessionRow } from './session-row';
import { SessionRowContextMenu, type SessionRowAction } from './session-row-menu';

/** Rows rendered eagerly below this count; virtualization needs a tall list. */
const VIRTUALIZE_THRESHOLD = 60;
const ROW_HEIGHT = 46;
const HEADER_HEIGHT = 28;

/**
 * The virtualized rail body.
 *
 * Headers and session rows are both virtual items, so a collapsed group costs
 * the same as an expanded one. Below {@link VIRTUALIZE_THRESHOLD} rows the list
 * renders directly: virtualization hurts assistive technology and adds jitter
 * to short lists for no benefit.
 *
 * Keyboard focus stays on the listbox; `aria-activedescendant` names the
 * active row, which is what lets arrow keys move without stealing DOM focus
 * from the search field.
 */
export function SessionRail({
  rows,
  selectedId,
  focusedId,
  machineNames,
  workspaceNames,
  offlineMachines,
  multiSelected,
  listLabel,
  onFocusRow,
  onToggleGroup,
  onAction,
  onActivate,
  renderHeaderAction,
}: {
  rows: RailRow[];
  selectedId: string;
  focusedId: string | null;
  /**
   * Kept for layout parity with the toolbar. The rail's shape comes from
   * `rows`, which the caller already assembled for the current mode.
   */
  groupMode: RailGroupMode;
  machineNames: Map<string, string>;
  workspaceNames: Map<string, string>;
  offlineMachines: Set<string>;
  multiSelected: Set<string>;
  listLabel: string;
  onFocusRow: (sessionId: string) => void;
  onToggleGroup: (groupKey: string) => void;
  onAction: (session: AgentSession, action: SessionRowAction) => void;
  onActivate: (session: AgentSession, event: React.MouseEvent) => void;
  renderHeaderAction?: (groupKey: string) => React.ReactNode;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const shouldVirtualize = rows.length > VIRTUALIZE_THRESHOLD;

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => (rows[index]?.kind === 'header' ? HEADER_HEIGHT : ROW_HEIGHT),
    getItemKey: (index) => rows[index]?.key ?? index,
    overscan: 8,
    enabled: shouldVirtualize,
  });

  // Keep the active row visible as keyboard navigation moves it.
  useEffect(() => {
    if (!shouldVirtualize || !focusedId) return;
    const index = rows.findIndex((row) => row.kind === 'session' && row.session.id === focusedId);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: 'auto' });
  }, [focusedId, rows, shouldVirtualize, virtualizer]);

  const renderRow = (row: RailRow) => {
    if (row.kind === 'header') {
      return (
        <div className="flex h-7 items-center gap-1.5 pl-1 pr-1">
          <button
            type="button"
            onClick={() => onToggleGroup(row.key.replace('header:', ''))}
            aria-expanded={!row.collapsed}
            className="flex min-w-0 flex-1 items-center gap-1.5 rounded text-left font-mono text-[9px] font-bold uppercase tracking-[0.1em] text-faint transition-colors hover:text-foreground"
          >
            <span
              aria-hidden="true"
              className={cn(
                'inline-block text-[8px] transition-transform',
                row.collapsed ? '-rotate-90' : 'rotate-0',
              )}
            >
              ▼
            </span>
            <span className="truncate">{row.label}</span>
            <span className="font-normal text-faint">{row.count}</span>
          </button>
          {renderHeaderAction?.(row.key.replace('header:', ''))}
        </div>
      );
    }

    const session = row.session;
    return (
      <div
        id={`session-option-${session.id}`}
        role="option"
        aria-selected={session.id === selectedId}
        tabIndex={-1}
        onFocus={() => onFocusRow(session.id)}
      >
        <SessionRowContextMenu session={session} onAction={onAction}>
          <span className="block">
            <SessionRow
              session={session}
              selected={session.id === selectedId}
              machineName={machineNames.get(session.machine_id) ?? 'Unknown machine'}
              workspaceName={
                session.workspace_id ? workspaceNames.get(session.workspace_id) : undefined
              }
              offline={offlineMachines.has(session.machine_id)}
              multiSelected={multiSelected.has(session.id)}
              onActivate={(target, event) => {
                onFocusRow(target.id);
                onActivate(target, event);
              }}
              onContextMenu={() => onFocusRow(session.id)}
            />
          </span>
        </SessionRowContextMenu>
      </div>
    );
  };

  const listProps = {
    role: 'listbox' as const,
    'aria-label': listLabel,
    'aria-activedescendant': focusedId ? `session-option-${focusedId}` : undefined,
    className: 'min-h-0 flex-1 overflow-y-auto px-1 pb-3',
  };

  if (!shouldVirtualize) {
    return (
      <div ref={scrollRef} {...listProps}>
        {rows.map((row) => (
          <div key={row.key}>{renderRow(row)}</div>
        ))}
      </div>
    );
  }

  return (
    <div ref={scrollRef} {...listProps}>
      <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
        {virtualizer.getVirtualItems().map((item) => (
          <div
            key={item.key}
            ref={virtualizer.measureElement}
            data-index={item.index}
            style={{
              position: 'absolute',
              top: 0,
              left: 0,
              width: '100%',
              transform: `translateY(${item.start}px)`,
            }}
          >
            {renderRow(rows[item.index])}
          </div>
        ))}
      </div>
    </div>
  );
}

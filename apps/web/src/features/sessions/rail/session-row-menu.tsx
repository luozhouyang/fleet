import type { AgentSession } from '@agent-fleet/protocol-types';
import {
  Archive,
  ArchiveRestore,
  CheckCheck,
  Mail,
  Pin,
  PinOff,
  Pencil,
  Trash2,
  X,
} from 'lucide-react';
import type { ReactElement } from 'react';
import { MenuItem, MenuSeparator } from '@/components/ui/menu';
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from '@/components/ui/context-menu';

export type SessionRowAction =
  | 'pin'
  | 'unpin'
  | 'archive'
  | 'unarchive'
  | 'mark-read'
  | 'mark-unread'
  | 'rename'
  | 'close'
  | 'delete';

/**
 * Wraps one session row with its context menu.
 *
 * The trigger is the row itself, so right-click, the keyboard's menu key and
 * touch long-press all open the same Base UI menu with correct focus handling.
 */
export function SessionRowContextMenu({
  session,
  onAction,
  children,
}: {
  session: AgentSession;
  onAction: (session: AgentSession, action: SessionRowAction) => void;
  children: ReactElement<Record<string, unknown>>;
}) {
  const pinned = Boolean(session.ui?.pinned);
  const archived = Boolean(session.ui?.archived);
  const unread = Boolean(session.ui?.unread);
  const terminal = session.status === 'terminated' || session.status === 'completed';

  return (
    <ContextMenu>
      <ContextMenuTrigger render={children} />
      <ContextMenuContent>
        <MenuItem
          icon={pinned ? <PinOff /> : <Pin />}
          onClick={() => onAction(session, pinned ? 'unpin' : 'pin')}
        >
          {pinned ? 'Unpin' : 'Pin'}
        </MenuItem>
        <MenuItem
          icon={unread ? <CheckCheck /> : <Mail />}
          onClick={() => onAction(session, unread ? 'mark-read' : 'mark-unread')}
        >
          {unread ? 'Mark read' : 'Mark unread'}
        </MenuItem>
        <MenuItem
          icon={archived ? <ArchiveRestore /> : <Archive />}
          onClick={() => onAction(session, archived ? 'unarchive' : 'archive')}
        >
          {archived ? 'Unarchive' : 'Archive'}
        </MenuItem>
        <MenuSeparator />
        <MenuItem icon={<Pencil />} onClick={() => onAction(session, 'rename')}>
          Rename
        </MenuItem>
        <MenuItem icon={<X />} disabled={terminal} onClick={() => onAction(session, 'close')}>
          Close session
        </MenuItem>
        <MenuItem icon={<Trash2 />} danger onClick={() => onAction(session, 'delete')}>
          Delete session
        </MenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

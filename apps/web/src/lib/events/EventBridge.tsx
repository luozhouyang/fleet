import type { EventConnectionState } from '@agent-fleet/api-client';
import type {
  AgentEvent,
  LagPayload,
  Machine,
  MachineStatusEvent,
  WorkspaceEvent,
} from '@agent-fleet/protocol-types';
import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { eventClient } from '@/lib/api/client';
import { queryKeys } from '@/lib/query/keys';
import { queueAgentEvents, useSessionStore } from '@/lib/session-store';
import { reportLag } from './lag-store';
import { useEventConnectionState } from './useEventConnection';

const SESSION_STATUS_EVENTS = new Set([
  'session.created',
  'session.started',
  'session.resumed',
  'session.terminated',
  'session.deleted',
  'session.error',
  'agent.status.changed',
]);

/** Single application-level bridge between platform events and query/store state. */
export function EventBridge() {
  const queryClient = useQueryClient();
  const connectionState = useEventConnectionState();
  const previousConnectionState = useRef<EventConnectionState | null>(null);

  // The gateway does not replay events missed while the socket was down, so a
  // reconnect is the recovery point: refetch every live view once the socket
  // comes back. The initial connect is already covered by the hooks' mount
  // fetches, so only a transition out of a lost connection triggers a refetch.
  useEffect(() => {
    const previous = previousConnectionState.current;
    previousConnectionState.current = connectionState;
    if (connectionState !== 'connected') return;
    if (previous !== 'disconnected' && previous !== 'reconnecting' && previous !== 'failed') {
      return;
    }

    void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
    void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
    void queryClient.invalidateQueries({ queryKey: queryKeys.workspacesRoot() });
  }, [connectionState, queryClient]);

  // A browser can cross an offline period or freeze a background tab without
  // ever seeing the socket close, so events may have been missed while the
  // connection still looks healthy. Refetch the live views when the network
  // returns or the tab becomes visible, and reconnect if the socket is down.
  useEffect(() => {
    const resync = () => {
      if (eventClient.getState() !== 'connected') {
        void eventClient.reconnect();
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.workspacesRoot() });
    };
    const handleOnline = () => resync();
    const handleVisibility = () => {
      if (document.visibilityState === 'visible') resync();
    };
    window.addEventListener('online', handleOnline);
    document.addEventListener('visibilitychange', handleVisibility);
    return () => {
      window.removeEventListener('online', handleOnline);
      document.removeEventListener('visibilitychange', handleVisibility);
    };
  }, [queryClient]);

  useEffect(() => {
    const unsubscribeAgent = eventClient.subscribe<AgentEvent>('agent.event', (event) => {
      const agentEvent = event.payload;
      if (!agentEvent?.session_id) return;
      if (agentEvent.type === 'session.deleted') {
        // The session row is gone; keeping its in-memory transcript would
        // leave a ghost until reload.
        useSessionStore.getState().removeSession(agentEvent.session_id);
        return;
      }
      queueAgentEvents(agentEvent);

      if (SESSION_STATUS_EVENTS.has(agentEvent.type)) {
        void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
        void queryClient.invalidateQueries({
          queryKey: queryKeys.session(agentEvent.session_id),
        });
      }
    });

    const unsubscribeMachine = eventClient.subscribe<MachineStatusEvent>(
      'machine.status',
      (event) => {
        const payload = event.payload;
        const machineId = event.machine_id ?? payload?.id;
        if (!machineId || !payload?.status) return;
        // The gateway may emit a compact `{id, status}` fallback instead of a
        // full snapshot. Merge it over the cached machine so a fallback event
        // cannot erase name, OS, workspaces or providers.
        const patch: Partial<Machine> = {
          ...payload,
          id: machineId,
          last_seen_at: payload.last_seen_at ?? event.timestamp,
          updated_at: payload.updated_at ?? event.timestamp,
        };

        queryClient.setQueryData<Machine[]>(queryKeys.machines(), (machines) =>
          machines?.map((item) => (item.id === machineId ? { ...item, ...patch } : item)),
        );
        queryClient.setQueryData<Machine>(queryKeys.machine(machineId), (machine) =>
          machine ? { ...machine, ...patch } : machine,
        );
        // The admin projects a disconnect into its session rows (and clears
        // the projection on reconciliation), so refetch the session views
        // rather than waiting for a polling interval.
        void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      },
    );

    const unsubscribeGit = eventClient.subscribe('git.status.changed', (event) => {
      const workspaceId = event.workspace_id;
      if (!workspaceId) return;
      void queryClient.invalidateQueries({
        queryKey: ['workspaces', workspaceId, 'git'],
      });
      void queryClient.invalidateQueries({
        queryKey: ['workspaces', workspaceId, 'tree'],
      });
    });

    const unsubscribeWorkspace = eventClient.subscribe<WorkspaceEvent>(
      'workspace.event',
      (event) => {
        const workspaceId = event.workspace_id ?? event.payload?.workspace_id;
        if (!workspaceId) return;
        // Refresh open file tabs so externally edited files do not go stale.
        void queryClient.invalidateQueries({
          queryKey: ['workspaces', workspaceId, 'file'],
        });
        void queryClient.invalidateQueries({
          queryKey: ['workspaces', workspaceId, 'tree'],
        });
        void queryClient.invalidateQueries({
          queryKey: ['workspaces', workspaceId, 'git'],
        });
      },
    );

    // The gateway dropped events on this connection. Refetch every live view
    // and surface a dismissible notice rather than silently showing stale data.
    const unsubscribeLag = eventClient.subscribe<LagPayload>('platform.lagged', (event) => {
      reportLag(event.payload?.skipped ?? 0);
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.workspaces() });
    });

    return () => {
      unsubscribeAgent();
      unsubscribeMachine();
      unsubscribeGit();
      unsubscribeWorkspace();
      unsubscribeLag();
    };
  }, [queryClient]);

  return null;
}

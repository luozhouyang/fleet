import type {
  AgentEvent,
  Machine,
  MachineStatusEvent,
  WorkspaceEvent,
} from '@agent-fleet/protocol-types';
import { useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { eventClient } from '@/lib/api/client';
import { queryKeys } from '@/lib/query/keys';
import { queueAgentEvents } from '@/lib/session-store';

const SESSION_STATUS_EVENTS = new Set([
  'session.created',
  'session.started',
  'session.resumed',
  'session.terminated',
  'session.error',
  'agent.status.changed',
]);

/** Single application-level bridge between platform events and query/store state. */
export function EventBridge() {
  const queryClient = useQueryClient();

  useEffect(() => {
    const unsubscribeAgent = eventClient.subscribe<AgentEvent>('agent.event', (event) => {
      const agentEvent = event.payload;
      if (!agentEvent?.session_id) return;
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
        const machine: Machine = {
          ...payload,
          id: payload.id,
          last_seen_at: payload.last_seen_at ?? event.timestamp,
          updated_at: payload.updated_at ?? event.timestamp,
        };

        queryClient.setQueryData<Machine[]>(queryKeys.machines(), (machines) =>
          machines?.map((item) => (item.id === machineId ? machine : item)),
        );
        queryClient.setQueryData<Machine>(queryKeys.machine(machineId), machine);
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
        // Open file content is intentionally not invalidated here. The file
        // viewer shows a manual reload banner instead of overwriting the view.
        void queryClient.invalidateQueries({
          queryKey: ['workspaces', workspaceId, 'tree'],
        });
        void queryClient.invalidateQueries({
          queryKey: ['workspaces', workspaceId, 'git'],
        });
      },
    );

    const unsubscribeConnection = eventClient.subscribe('connection.state', () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
    });

    return () => {
      unsubscribeAgent();
      unsubscribeMachine();
      unsubscribeGit();
      unsubscribeWorkspace();
      unsubscribeConnection();
    };
  }, [queryClient]);

  return null;
}

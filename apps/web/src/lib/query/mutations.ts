import type {
  AgentSession,
  BulkUpdateSessionUiRequest,
  Machine,
  SessionFolder,
  UpdateSessionUiRequest,
  Workspace,
} from '@agent-fleet/protocol-types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '@/lib/api/client';
import { queryKeys } from './keys';

function useInvalidateWorkspace(workspaceId: string) {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.gitStatus(workspaceId) });
    void queryClient.invalidateQueries({ queryKey: ['workspaces', workspaceId, 'git'] });
    void queryClient.invalidateQueries({ queryKey: ['workspaces', workspaceId, 'tree'] });
  };
}

export function useGitStage(workspaceId: string) {
  const invalidate = useInvalidateWorkspace(workspaceId);
  return useMutation({
    mutationFn: (paths: string[]) => apiClient.gitStage(workspaceId, { paths }),
    onSuccess: invalidate,
  });
}

export function useGitUnstage(workspaceId: string) {
  const invalidate = useInvalidateWorkspace(workspaceId);
  return useMutation({
    mutationFn: (paths: string[]) => apiClient.gitUnstage(workspaceId, { paths }),
    onSuccess: invalidate,
  });
}

export function useGitCommit(workspaceId: string) {
  const invalidate = useInvalidateWorkspace(workspaceId);
  return useMutation({
    mutationFn: (input: { message: string; paths?: string[]; idempotencyKey?: string }) =>
      apiClient.gitCommit(
        workspaceId,
        { message: input.message, paths: input.paths },
        { idempotencyKey: input.idempotencyKey },
      ),
    onSuccess: invalidate,
  });
}

export function useGitCheckout(workspaceId: string) {
  const invalidate = useInvalidateWorkspace(workspaceId);
  return useMutation({
    mutationFn: (input: { branch: string; create?: boolean; idempotencyKey?: string }) =>
      apiClient.gitCheckout(
        workspaceId,
        { branch: input.branch, create: input.create },
        { idempotencyKey: input.idempotencyKey },
      ),
    onSuccess: invalidate,
  });
}

export function useCreateWorkspace() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { machineId: string; name?: string; rootPath: string }) =>
      apiClient.createWorkspace({
        machine_id: input.machineId,
        name: input.name,
        root_path: input.rootPath,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.workspaces() });
    },
  });
}

export function useDeleteWorkspace() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (workspace: Pick<Workspace, 'id'>) => apiClient.deleteWorkspace(workspace.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.workspaces() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
    },
  });
}

export function useUpdateSessionTitle() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ sessionId, title }: { sessionId: string; title: string | null }) =>
      apiClient.updateSession(sessionId, { title }),
    onSuccess: (session) => {
      queryClient.setQueryData<AgentSession[]>(queryKeys.sessionList(true), (sessions) =>
        sessions?.map((item) => (item.id === session.id ? { ...item, ...session } : item)),
      );
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
    },
  });
}

/** Mark sessions read up to the observed `updated_at`. */
export function useMarkSessionsRead() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ids, watermark }: { ids: string[]; watermark: string }) =>
      apiClient.markSessionsRead(ids, watermark),
    onSuccess: () => {
      // Unread is derived server-side; refetch rather than guess.
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
    },
  });
}

/** Mark one session unread again for the caller. */
export function useMarkSessionUnread() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (sessionId: string) => apiClient.markSessionUnread(sessionId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['sessions'] });
    },
  });
}

/** Update the caller's presentation state for one session. */
export function useUpdateSessionUi() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ sessionId, patch }: { sessionId: string; patch: UpdateSessionUiRequest }) =>
      apiClient.updateSessionUi(sessionId, patch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessionFacets() });
      void queryClient.invalidateQueries({ queryKey: ['sessions', 'rail'] });
    },
  });
}

/** Apply one presentation patch to several sessions. */
export function useUpdateSessionsUi() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: BulkUpdateSessionUiRequest) => apiClient.updateSessionsUi(request),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessionFacets() });
      void queryClient.invalidateQueries({ queryKey: ['sessions', 'rail'] });
    },
  });
}

/** Persist the pinned order. */
export function useReorderPins() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (ids: string[]) => apiClient.reorderPinnedSessions(ids),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['sessions', 'rail'] });
    },
  });
}

export function useCreateSessionFolder() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => apiClient.createSessionFolder(name),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessionFolders() });
    },
  });
}

export function useRenameSessionFolder() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) =>
      apiClient.renameSessionFolder(id, name),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessionFolders() });
    },
  });
}

export function useDeleteSessionFolder() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiClient.deleteSessionFolder(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessionFolders() });
      void queryClient.invalidateQueries({ queryKey: ['sessions', 'rail'] });
    },
  });
}

/** One folder, used by tests and callers that create and then assign. */
export type { SessionFolder };

export function useUnrevokeMachine() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (machineId: string) => apiClient.unrevokeMachine(machineId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.revokedMachines() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
    },
  });
}

export function useRotateMachineToken() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (machineId: string) => apiClient.rotateMachineToken(machineId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
    },
  });
}

export function useDeleteMachine() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (machine: Machine) => apiClient.deleteMachine(machine.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.machines() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.workspaces() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions() });
    },
  });
}

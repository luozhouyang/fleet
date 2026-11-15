import type { Machine, Workspace } from '@agent-fleet/protocol-types';
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
    mutationFn: (input: { message: string; paths?: string[] }) =>
      apiClient.gitCommit(workspaceId, input),
    onSuccess: invalidate,
  });
}

export function useGitCheckout(workspaceId: string) {
  const invalidate = useInvalidateWorkspace(workspaceId);
  return useMutation({
    mutationFn: (input: { branch: string; create?: boolean }) =>
      apiClient.gitCheckout(workspaceId, input),
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

import type { GitDiffOptions } from '@agent-fleet/protocol-types';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '@/lib/api/client';
import { queryKeys } from './keys';

export function useMachines() {
  return useQuery({
    queryKey: queryKeys.machines(),
    queryFn: ({ signal }) => apiClient.listMachines({ signal }),
    refetchInterval: 15_000,
  });
}

export function useMachine(id: string) {
  return useQuery({
    queryKey: queryKeys.machine(id),
    queryFn: ({ signal }) => apiClient.getMachine(id, { signal }),
    enabled: Boolean(id),
  });
}

export function useMachineProviders(id: string, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.machineProviders(id),
    queryFn: ({ signal }) => apiClient.getMachineProviders(id, { signal }),
    enabled: Boolean(id) && enabled,
    retry: false,
    staleTime: 60_000,
  });
}

export function useWorkspacesForMachine(machineId: string) {
  return useQuery({
    queryKey: queryKeys.workspacesForMachine(machineId),
    queryFn: ({ signal }) => apiClient.listWorkspaces(machineId, { signal }),
    enabled: Boolean(machineId),
    retry: false,
  });
}

export function useGitStatus(workspaceId: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.gitStatus(workspaceId),
    queryFn: ({ signal }) => apiClient.gitStatus(workspaceId, { signal }),
    enabled: Boolean(workspaceId) && enabled,
    retry: false,
    refetchInterval: 30_000,
  });
}

export function useGitDiffSummary(workspaceId: string, options: GitDiffOptions, enabled = true) {
  return useQuery({
    queryKey: queryKeys.gitDiffSummary(workspaceId, options),
    queryFn: ({ signal }) => apiClient.gitDiffSummary(workspaceId, options, { signal }),
    enabled: Boolean(workspaceId) && enabled,
    retry: false,
  });
}

export function useGitDiff(workspaceId: string, options: GitDiffOptions, enabled = true) {
  return useQuery({
    queryKey: queryKeys.gitDiff(workspaceId, options),
    queryFn: ({ signal }) => apiClient.gitDiff(workspaceId, options, { signal }),
    enabled: Boolean(workspaceId) && enabled,
    retry: false,
  });
}

export function useSessionList() {
  return useQuery({
    queryKey: queryKeys.sessions(),
    queryFn: ({ signal }) => apiClient.listSessions({ signal }),
    refetchInterval: 15_000,
  });
}

export function useSessionDetail(id: string) {
  return useQuery({
    queryKey: queryKeys.session(id),
    queryFn: ({ signal }) => apiClient.getSession(id, { signal }),
    enabled: Boolean(id),
    refetchInterval: 20_000,
  });
}

export function useSessionPermissions(id: string) {
  return useQuery({
    queryKey: queryKeys.sessionPermissions(id),
    queryFn: ({ signal }) => apiClient.listPermissions(id, { signal }),
    enabled: Boolean(id),
    retry: false,
  });
}

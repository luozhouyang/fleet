import type { FileContent, GitDiffOptions, Machine, Workspace } from '@agent-fleet/protocol-types';
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo } from 'react';
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
    refetchInterval: 15_000,
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

export interface WorkspaceLocation {
  machine: Machine;
  workspace: Workspace;
}

/** Workspaces are embedded in the machine registry; flatten them for index pages. */
export function useWorkspaceIndex() {
  const machinesQuery = useMachines();
  const locations = useMemo<WorkspaceLocation[]>(() => {
    const result: WorkspaceLocation[] = [];
    for (const machine of machinesQuery.data ?? []) {
      for (const summary of machine.workspaces ?? []) {
        result.push({
          machine,
          workspace: {
            id: summary.id,
            machine_id: machine.id,
            name: summary.name,
            root_path: summary.root_path,
            is_git_repository: summary.is_git_repository,
            created_at: machine.created_at,
            updated_at: machine.updated_at,
          },
        });
      }
    }
    return result.sort(
      (left, right) =>
        left.machine.name.localeCompare(right.machine.name) ||
        left.workspace.name.localeCompare(right.workspace.name),
    );
  }, [machinesQuery.data]);

  return { ...machinesQuery, locations };
}

export function useWorkspaceLocation(workspaceId: string) {
  const index = useWorkspaceIndex();
  return useMemo(
    () => ({
      ...index,
      location: index.locations.find((entry) => entry.workspace.id === workspaceId),
    }),
    [index, workspaceId],
  );
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

export function useGitBranches(workspaceId: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.gitBranches(workspaceId),
    queryFn: ({ signal }) => apiClient.gitBranches(workspaceId, { signal }),
    enabled: Boolean(workspaceId) && enabled,
    retry: false,
    staleTime: 10_000,
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

export function useGitLog(workspaceId: string, reference?: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.gitLog(workspaceId, reference),
    queryFn: ({ signal }) => apiClient.gitLog(workspaceId, { limit: 30, reference }, { signal }),
    enabled: Boolean(workspaceId) && enabled,
    retry: false,
    staleTime: 10_000,
  });
}

export function useDirectory(
  workspaceId: string,
  path: string,
  showHidden: boolean,
  enabled = true,
) {
  return useInfiniteQuery({
    queryKey: queryKeys.tree(workspaceId, path, showHidden),
    queryFn: ({ signal, pageParam }) =>
      apiClient.getTree(
        workspaceId,
        {
          path: path || undefined,
          cursor: pageParam,
          show_ignored: showHidden,
          limit: 200,
        },
        { signal },
      ),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor,
    enabled: Boolean(workspaceId) && enabled,
    retry: false,
    staleTime: 5_000,
  });
}

export function useFileContent(workspaceId: string, path: string, enabled = true) {
  const queryClient = useQueryClient();
  const queryKey = queryKeys.file(workspaceId, path);
  return useQuery({
    queryKey,
    queryFn: async ({ signal }) => {
      const result = await apiClient.readFile(workspaceId, path, { signal });
      if ('content' in result) return result;
      const cached = queryClient.getQueryData<FileContent>(queryKey);
      if (cached) return cached;
      throw new Error('File was not modified and is not cached.');
    },
    enabled: Boolean(workspaceId) && Boolean(path) && enabled,
    retry: false,
  });
}

export function useWorkspaceSearch(
  workspaceId: string,
  query: string,
  path: string,
  enabled = true,
) {
  return useQuery({
    queryKey: queryKeys.search(workspaceId, query, path),
    queryFn: ({ signal }) =>
      apiClient.search(
        workspaceId,
        { query, path: path || undefined, max_results: 100 },
        { signal },
      ),
    enabled: Boolean(workspaceId) && query.trim().length > 1 && enabled,
    retry: false,
    staleTime: 5_000,
  });
}

export function useRevokedMachines() {
  return useQuery({
    queryKey: queryKeys.revokedMachines(),
    queryFn: ({ signal }) => apiClient.listRevokedMachines({ signal }),
    retry: false,
    staleTime: 30_000,
  });
}

/**
 * Lists sessions. Closed (terminated/archived) sessions are excluded by
 * default; pass `includeClosed` to receive them for the Closed filter.
 */
export function useSessionList(includeClosed = false) {
  return useQuery({
    queryKey: queryKeys.sessionList(includeClosed),
    queryFn: ({ signal }) => apiClient.listSessions({ signal, includeClosed }),
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

export function useSessionQuestions(id: string) {
  return useQuery({
    queryKey: queryKeys.sessionQuestions(id),
    queryFn: ({ signal }) => apiClient.listQuestions(id, { signal }),
    enabled: Boolean(id),
    retry: false,
  });
}

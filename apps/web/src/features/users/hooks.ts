import type { CreateUserRequest } from '@agent-fleet/protocol-types';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '@/lib/api/client';

export const userKeys = {
  me: () => ['me'] as const,
  usersRoot: () => ['users'] as const,
  users: (orgId: string) => ['users', orgId || 'default'] as const,
  membersRoot: () => ['org-members'] as const,
  members: (orgId: string) => ['org-members', orgId || 'default'] as const,
  organizations: () => ['organizations'] as const,
};

/** Current principal; shared by the nav, switcher, settings and expiry banner. */
export function useMe() {
  return useQuery({
    queryKey: userKeys.me(),
    queryFn: ({ signal }) => apiClient.me({ signal }),
    staleTime: 30_000,
  });
}

/**
 * Accounts visible to the caller, including token lifetimes.
 *
 * The server scopes the list to the active organization; `orgId` is passed
 * explicitly so an in-flight request cannot land after a switch and be filed
 * under the new organization's cache entry.
 */
export function useUsers(orgId: string, enabled = true) {
  return useQuery({
    queryKey: userKeys.users(orgId),
    queryFn: ({ signal }) => apiClient.listUsers(orgId || null, { signal }),
    enabled,
    retry: false,
  });
}

/** Members of one organization, including their role there. */
export function useOrganizationMembers(orgId: string, enabled = true) {
  return useQuery({
    queryKey: userKeys.members(orgId),
    queryFn: ({ signal }) => apiClient.listOrganizationMembers(orgId, { signal }),
    enabled: enabled && Boolean(orgId),
    retry: false,
  });
}

/** Organizations the caller belongs to, with its role in each. */
export function useOrganizations(enabled = true) {
  return useQuery({
    queryKey: userKeys.organizations(),
    queryFn: ({ signal }) => apiClient.listOrganizations({ signal }),
    enabled,
    staleTime: 60_000,
    retry: false,
  });
}

export function useCreateUser() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: CreateUserRequest) => apiClient.createUser(request),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: userKeys.usersRoot() });
      void queryClient.invalidateQueries({ queryKey: userKeys.membersRoot() });
    },
  });
}

export function useDeleteUser() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiClient.deleteUser(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: userKeys.usersRoot() });
      void queryClient.invalidateQueries({ queryKey: userKeys.membersRoot() });
    },
  });
}

export function useDisableUser() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiClient.disableUser(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: userKeys.usersRoot() });
      void queryClient.invalidateQueries({ queryKey: userKeys.membersRoot() });
    },
  });
}

export function useEnableUser() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiClient.enableUser(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: userKeys.usersRoot() });
      void queryClient.invalidateQueries({ queryKey: userKeys.membersRoot() });
    },
  });
}

/**
 * Rotate any user's token.
 *
 * Invalidation is deliberately left to the caller: when a user rotates their
 * own token, the old credential is invalid immediately, so the new one must be
 * stored before any query refetches.
 */
export function useRotateUserToken() {
  return useMutation({
    mutationFn: (input: { userId: string; expiresInDays?: number | null }) =>
      apiClient.rotateUserToken(input.userId, input.expiresInDays),
  });
}

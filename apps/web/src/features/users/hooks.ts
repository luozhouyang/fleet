import type { CreateUserRequest } from '@agent-fleet/protocol-types';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '@/lib/api/client';

export const userKeys = {
  me: () => ['me'] as const,
  users: () => ['users'] as const,
  organizations: () => ['organizations'] as const,
};

/** Current principal; shared by the nav, settings and the expiry banner. */
export function useMe() {
  return useQuery({
    queryKey: userKeys.me(),
    queryFn: ({ signal }) => apiClient.me({ signal }),
    staleTime: 30_000,
  });
}

/** Users visible to the caller. Disabled until the role is known to be admin. */
export function useUsers(enabled = true) {
  return useQuery({
    queryKey: userKeys.users(),
    queryFn: ({ signal }) => apiClient.listUsers({ signal }),
    enabled,
    retry: false,
  });
}

/** Organizations visible to the caller. Administrators only. */
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
      void queryClient.invalidateQueries({ queryKey: userKeys.users() });
    },
  });
}

export function useDeleteUser() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiClient.deleteUser(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: userKeys.users() });
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

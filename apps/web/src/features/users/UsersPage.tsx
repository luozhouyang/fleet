import type { User } from '@agent-fleet/protocol-types';
import { KeyRound, RefreshCw, Trash2, UserPlus, UsersRound } from 'lucide-react';
import { useMemo, useState } from 'react';
import { ConfirmDialog } from '@/components/dialogs/confirm-dialog';
import { Button } from '@/components/ui/button';
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/feedback';
import { useActiveOrgId } from '@/lib/api/organization';
import { formatDateTime, getErrorMessage, shortId } from '@/lib/utils/format';
import { CreateUserDialog } from './CreateUserDialog';
import { useDeleteUser, useMe, useOrganizationMembers, useUsers } from './hooks';
import { RotateTokenDialog } from './RotateTokenDialog';
import { PageKicker, Panel } from './user-ui';
import {
  ROLE_LABELS,
  roleIsAdmin,
  tokenExpiryLabel,
  tokenIsExpired,
  tokenIsExpiringSoon,
  usersPageContainer,
} from './user-utils';

export function UsersPage() {
  const meQuery = useMe();
  const storedOrgId = useActiveOrgId();
  const orgId = storedOrgId || meQuery.data?.org_id || '';
  const isAdmin = roleIsAdmin(meQuery.data?.role);
  const usersQuery = useUsers(orgId, isAdmin);
  const membersQuery = useOrganizationMembers(orgId, isAdmin);
  const deleteMutation = useDeleteUser();

  const [createOpen, setCreateOpen] = useState(false);
  const [rotateTarget, setRotateTarget] = useState<User | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<User | null>(null);

  const users = usersQuery.data ?? [];
  const selfId = meQuery.data?.user?.id;

  // Roles live on memberships, not on the global account.
  const rolesByUser = useMemo(
    () =>
      new Map((membersQuery.data ?? []).map((member) => [member.user_id, member.role] as const)),
    [membersQuery.data],
  );

  const expiringSoon = users.filter(
    (user) => tokenIsExpiringSoon(user.token_expires_at) || tokenIsExpired(user.token_expires_at),
  ).length;

  if (meQuery.isLoading) {
    return (
      <div className="h-full overflow-y-auto">
        <div className={usersPageContainer}>
          <Panel>
            <LoadingState label="Loading identity…" />
          </Panel>
        </div>
      </div>
    );
  }

  if (meQuery.isError || !meQuery.data) {
    return (
      <div className="h-full overflow-y-auto">
        <div className={usersPageContainer}>
          <Panel>
            <ErrorState
              title="Could not load your identity"
              message={getErrorMessage(meQuery.error)}
              onRetry={() => void meQuery.refetch()}
            />
          </Panel>
        </div>
      </div>
    );
  }

  if (!isAdmin) {
    return (
      <div className="h-full overflow-y-auto">
        <div className={usersPageContainer}>
          <Panel>
            <EmptyState
              title="Administrators only"
              description="Your token does not have the admin role, so user management is unavailable."
              icon={<UsersRound className="size-5" aria-hidden="true" />}
            />
          </Panel>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full overflow-y-auto">
      <div className={usersPageContainer}>
        <header className="flex flex-wrap items-end justify-between gap-4">
          <div className="flex min-w-0 flex-col gap-1">
            <PageKicker>FLEET / USERS</PageKicker>
            <h1 className="font-display text-[30px] font-bold leading-9 tracking-[-0.02em] text-foreground">
              Users
            </h1>
            <p className="text-[12.5px] leading-5 text-muted-foreground">
              {usersQuery.isLoading
                ? 'Loading users…'
                : `${users.length} ${users.length === 1 ? 'user' : 'users'} · ${expiringSoon} needing token attention`}
            </p>
          </div>
          <Button
            variant="primary"
            onClick={() => setCreateOpen(true)}
            disabled={!orgId}
            title={orgId ? 'Create a user' : 'Select an organization first'}
          >
            <UserPlus className="size-3.5" aria-hidden="true" />
            Create user
          </Button>
        </header>

        {usersQuery.isError ? (
          <Panel>
            <ErrorState
              title="Could not load users"
              message={getErrorMessage(usersQuery.error)}
              onRetry={() => void usersQuery.refetch()}
            />
          </Panel>
        ) : usersQuery.isLoading ? (
          <Panel>
            <LoadingState label="Loading users…" />
          </Panel>
        ) : users.length === 0 ? (
          <Panel>
            <EmptyState
              title="No users yet"
              description="Create a user to issue a scoped bearer token for a member or viewer."
              icon={<UsersRound className="size-5" aria-hidden="true" />}
              action={
                <Button variant="secondary" size="sm" onClick={() => setCreateOpen(true)}>
                  Create user
                </Button>
              }
            />
          </Panel>
        ) : (
          <Panel>
            <div className="overflow-x-auto">
              <table className="w-full min-w-[860px] border-collapse text-left">
                <thead>
                  <tr className="h-9 bg-muted">
                    {['Name', 'Email', 'Role', 'Token issued', 'Token expires', 'Actions'].map(
                      (heading) => (
                        <th
                          key={heading}
                          scope="col"
                          className={
                            heading === 'Actions'
                              ? 'px-5 text-right font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint'
                              : 'px-5 font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint'
                          }
                        >
                          {heading}
                        </th>
                      ),
                    )}
                  </tr>
                </thead>
                <tbody>
                  {users.map((user) => {
                    const expired = tokenIsExpired(user.token_expires_at);
                    const expiring = tokenIsExpiringSoon(user.token_expires_at);
                    const isSelf = selfId === user.id;
                    const role = rolesByUser.get(user.id);
                    return (
                      <tr
                        key={user.id}
                        data-testid="user-row"
                        data-id={user.id}
                        className="h-16 border-t border-border"
                      >
                        <td className="px-5 py-2.5">
                          <span className="block truncate text-[13px] font-bold leading-5 text-foreground">
                            {user.name}
                            {isSelf ? (
                              <span className="ml-2 font-mono text-[9px] uppercase tracking-[0.1em] text-faint">
                                you
                              </span>
                            ) : null}
                          </span>
                          <span className="block truncate font-mono text-[9.5px] leading-4 text-faint">
                            {shortId(user.id)}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <span className="inline-flex rounded-full bg-muted px-2 py-0.5 font-mono text-[9px] font-medium uppercase tracking-[0.1em] text-muted-foreground">
                            {role ? ROLE_LABELS[role] : '—'}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <span className="block truncate text-[12px] text-muted-foreground">
                            {user.email ?? '—'}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <span className="font-mono text-[11px] text-muted-foreground">
                            {formatDateTime(user.token_issued_at)}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <span
                            className={
                              expired
                                ? 'font-mono text-[11px] font-bold text-destructive'
                                : expiring
                                  ? 'font-mono text-[11px] font-bold text-warning'
                                  : 'font-mono text-[11px] text-muted-foreground'
                            }
                            title={user.token_expires_at ?? 'Never expires'}
                          >
                            {tokenExpiryLabel(user.token_expires_at)}
                          </span>
                        </td>
                        <td className="px-5 py-2.5">
                          <div className="flex items-center justify-end gap-1.5">
                            <Button
                              variant="secondary"
                              size="sm"
                              onClick={() => setRotateTarget(user)}
                              aria-label={`Rotate token for ${user.name}`}
                            >
                              <RefreshCw className="size-3" aria-hidden="true" />
                              Rotate
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              className="text-destructive hover:text-destructive"
                              onClick={() => setDeleteTarget(user)}
                              disabled={isSelf}
                              title={isSelf ? 'Administrators cannot delete themselves' : undefined}
                              aria-label={`Delete ${user.name}`}
                            >
                              <Trash2 className="size-3" aria-hidden="true" />
                              Delete
                            </Button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </Panel>
        )}

        {membersQuery.isError ? (
          <p className="rounded-md border border-destructive/20 bg-destructive-soft px-3 py-2 text-[11.5px] text-destructive">
            Could not load organization members: {getErrorMessage(membersQuery.error)}
          </p>
        ) : null}

        {createOpen ? (
          <CreateUserDialog orgId={orgId} onClose={() => setCreateOpen(false)} />
        ) : null}

        {rotateTarget ? (
          <RotateTokenDialog
            userId={rotateTarget.id}
            userLabel={rotateTarget.name}
            self={selfId === rotateTarget.id}
            onClose={() => setRotateTarget(null)}
          />
        ) : null}

        <ConfirmDialog
          open={deleteTarget !== null}
          onOpenChange={(next) => {
            if (!next) setDeleteTarget(null);
          }}
          title={deleteTarget ? `Delete ${deleteTarget.name}?` : 'Delete user?'}
          description="The user's token stops working immediately. This cannot be undone."
          confirmLabel="Delete user"
          danger
          pending={deleteMutation.isPending}
          error={deleteMutation.error ? getErrorMessage(deleteMutation.error) : undefined}
          onConfirm={() => {
            if (!deleteTarget) return;
            deleteMutation.mutate(deleteTarget.id, {
              onSuccess: () => setDeleteTarget(null),
            });
          }}
        />

        <p className="flex items-center gap-1.5 text-[10.5px] leading-4 text-faint">
          <KeyRound className="size-3" aria-hidden="true" />
          Tokens are shown once. Rotating or deleting a user invalidates the previous token
          immediately.
        </p>
      </div>
    </div>
  );
}

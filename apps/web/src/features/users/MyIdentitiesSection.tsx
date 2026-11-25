import type { LinkedIdentity } from '@agent-fleet/protocol-types';
import { useState } from 'react';
import { ConfirmDialog } from '@/components/dialogs/confirm-dialog';
import { Button } from '@/components/ui/button';
import { formatDateTime, getErrorMessage } from '@/lib/utils/format';
import { useMe, useMyIdentities, useUnlinkIdentity } from './hooks';

const PROVIDER_LABELS: Record<string, string> = {
  github: 'GitHub',
  google: 'Google',
};

function providerLabel(provider: string): string {
  return PROVIDER_LABELS[provider] ?? provider;
}

/** Sign-in identities linked to the signed-in account. */
export function MyIdentitiesSection() {
  const meQuery = useMe();
  const isUser = meQuery.data?.kind === 'user';
  const identitiesQuery = useMyIdentities(isUser);
  const unlink = useUnlinkIdentity();
  const [target, setTarget] = useState<LinkedIdentity | null>(null);

  // The bootstrap admin has no account, so it has no identities to manage.
  if (!isUser) return null;

  const identities = identitiesQuery.data ?? [];
  const unlinkError = unlink.error ? getErrorMessage(unlink.error) : undefined;

  return (
    <section className="mt-6 border-t border-border pt-4">
      <h3 className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
        Sign-in methods
      </h3>
      <p className="mt-1 text-[12px] leading-5 text-muted-foreground">
        External identities linked to this account. Unlinking removes only the provider link;
        your machines, workspaces and sessions stay in place.
      </p>

      {identitiesQuery.isLoading ? (
        <p className="mt-3 text-[11.5px] text-faint">Loading sign-in methods…</p>
      ) : identitiesQuery.isError ? (
        <p className="mt-3 text-[11.5px] text-destructive">
          {getErrorMessage(identitiesQuery.error)}
        </p>
      ) : identities.length === 0 ? (
        <p className="mt-3 text-[11.5px] text-faint">
          No linked sign-in methods. This account signs in with its user token.
        </p>
      ) : (
        <ul className="mt-3 flex flex-col gap-2">
          {identities.map((identity) => (
            <li
              key={identity.id}
              className="flex items-center justify-between gap-3 rounded-md border border-border bg-muted/40 px-3 py-2"
            >
              <div className="min-w-0">
                <p className="flex items-center gap-2 text-[11.5px] text-foreground">
                  <span className="font-bold">{providerLabel(identity.provider)}</span>
                  <span className="truncate font-mono text-[10.5px] text-faint">
                    {identity.email}
                  </span>
                </p>
                <p className="mt-0.5 text-[10.5px] text-faint">
                  {identity.last_login_at
                    ? `Last sign-in ${formatDateTime(identity.last_login_at)}`
                    : `Linked ${formatDateTime(identity.created_at)}`}
                </p>
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="shrink-0 text-destructive"
                onClick={() => {
                  unlink.reset();
                  setTarget(identity);
                }}
              >
                Unlink
              </Button>
            </li>
          ))}
        </ul>
      )}

      {target ? (
        <ConfirmDialog
          open
          onOpenChange={(open) => {
            if (!open) setTarget(null);
          }}
          title={`Unlink ${providerLabel(target.provider)}?`}
          description={`${target.email} will no longer sign in to this account. You need another identity or a valid user token to keep accessing it.`}
          confirmLabel="Unlink"
          danger
          pending={unlink.isPending}
          error={unlinkError}
          onConfirm={() =>
            unlink.mutate(target.id, {
              onSuccess: () => setTarget(null),
            })
          }
        />
      ) : null}
    </section>
  );
}

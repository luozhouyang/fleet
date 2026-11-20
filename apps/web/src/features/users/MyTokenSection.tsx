import { RefreshCw } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { formatDateTime, getErrorMessage } from '@/lib/utils/format';
import { useMe } from './hooks';
import { RotateTokenDialog } from './RotateTokenDialog';
import {
  principalLabel,
  ROLE_LABELS,
  tokenExpiryLabel,
  tokenIsExpired,
  tokenIsExpiringSoon,
} from './user-utils';

/** Profile and token rotation for the signed-in principal. */
export function MyTokenSection() {
  const meQuery = useMe();
  const [rotateOpen, setRotateOpen] = useState(false);

  return (
    <section className="mt-6 border-t border-border pt-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 className="font-mono text-[9px] font-medium uppercase tracking-[0.14em] text-faint">
            My token
          </h3>
          <p className="mt-1 text-[12px] leading-5 text-muted-foreground">
            The credential this browser uses for REST and live events.
          </p>
        </div>
        {meQuery.data?.kind === 'user' && meQuery.data.user ? (
          <Button variant="secondary" size="sm" onClick={() => setRotateOpen(true)}>
            <RefreshCw className="size-3" aria-hidden="true" />
            Rotate token
          </Button>
        ) : null}
      </div>

      {meQuery.isLoading ? (
        <p className="mt-3 text-[11.5px] text-faint">Loading token details…</p>
      ) : meQuery.isError || !meQuery.data ? (
        <p className="mt-3 text-[11.5px] text-destructive">
          {getErrorMessage(meQuery.error)}
        </p>
      ) : meQuery.data.kind === 'legacy_admin' ? (
        <dl className="mt-3 grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[11.5px]">
          <dt className="text-faint">Principal</dt>
          <dd className="text-foreground">{principalLabel(meQuery.data.kind)}</dd>
          <dt className="text-faint">Role</dt>
          <dd className="text-foreground">{ROLE_LABELS[meQuery.data.role]}</dd>
          <dt className="text-faint">Expires</dt>
          <dd className="text-foreground">Never</dd>
          <dd className="col-span-2 mt-1 text-[10.5px] leading-4 text-faint">
            Configured admin token; rotates outside the UI.
          </dd>
        </dl>
      ) : meQuery.data.user ? (
        <dl className="mt-3 grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[11.5px]">
          <dt className="text-faint">Principal</dt>
          <dd className="text-foreground">
            {meQuery.data.user.name}{' '}
            <span className="font-mono text-[9.5px] text-faint">
              ({principalLabel(meQuery.data.kind)})
            </span>
          </dd>
          <dt className="text-faint">Role</dt>
          <dd className="text-foreground">{ROLE_LABELS[meQuery.data.role]}</dd>
          <dt className="text-faint">Issued</dt>
          <dd className="text-foreground">
            {formatDateTime(meQuery.data.user.token_issued_at)}
          </dd>
          <dt className="text-faint">Expires</dt>
          <dd
            className={
              tokenIsExpired(meQuery.data.user.token_expires_at)
                ? 'font-bold text-destructive'
                : tokenIsExpiringSoon(meQuery.data.user.token_expires_at)
                  ? 'font-bold text-warning'
                  : 'text-foreground'
            }
          >
            {tokenExpiryLabel(meQuery.data.user.token_expires_at)}
          </dd>
        </dl>
      ) : null}

      {rotateOpen && meQuery.data?.user ? (
        <RotateTokenDialog
          userId={meQuery.data.user.id}
          userLabel={meQuery.data.user.name}
          self
          onClose={() => setRotateOpen(false)}
        />
      ) : null}
    </section>
  );
}

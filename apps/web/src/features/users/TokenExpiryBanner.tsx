import { AlertTriangle } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { useMe } from './hooks';
import { RotateTokenDialog } from './RotateTokenDialog';
import { daysUntilExpiry } from './user-utils';

/**
 * Warn when the signed-in user's token expires within a week.
 *
 * Legacy admin tokens never expire and are excluded. Tokens that have already
 * expired are handled by the auth gate, so the banner stays hidden then.
 */
export function TokenExpiryBanner() {
  const meQuery = useMe();
  const [rotateOpen, setRotateOpen] = useState(false);

  const user = meQuery.data?.kind === 'user' ? meQuery.data.user : null;
  const days = daysUntilExpiry(user?.token_expires_at);
  if (!user || days === null || days <= 0 || days > 7) return null;

  return (
    <>
      <div
        role="status"
        data-testid="token-expiry-banner"
        className="flex shrink-0 items-center justify-between gap-3 border-b border-border bg-warning-soft px-5 py-2 text-[11px] text-warning"
      >
        <span className="flex items-center gap-1.5">
          <AlertTriangle className="size-3.5 shrink-0" aria-hidden="true" />
          Your API token {days === 1 ? 'expires tomorrow' : `expires in ${days} days`}. Rotate it
          to keep this browser connected.
        </span>
        <Button variant="secondary" size="sm" onClick={() => setRotateOpen(true)}>
          Rotate token
        </Button>
      </div>
      {rotateOpen ? (
        <RotateTokenDialog
          userId={user.id}
          userLabel={user.name}
          self
          onClose={() => setRotateOpen(false)}
        />
      ) : null}
    </>
  );
}

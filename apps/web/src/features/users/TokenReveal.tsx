import { Check, Copy } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { formatDateTime } from '@/lib/utils/format';

/**
 * One-time display of a freshly created or rotated token.
 *
 * The token is held only in component state and is never refetched or stored.
 */
export function TokenReveal({
  token,
  expiresAt,
  onAdopt,
}: {
  token: string;
  expiresAt?: string | null;
  onAdopt?: () => void;
}) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(token);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="mt-4 flex flex-col gap-3">
      <p className="rounded-md border border-warning/30 bg-warning-soft px-3 py-2 text-[11.5px] leading-5 text-warning">
        This token is shown once and cannot be recovered. Copy it now.
      </p>
      <div className="flex items-center gap-1.5">
        <code
          data-testid="revealed-token"
          className="min-w-0 flex-1 overflow-x-auto rounded-md border border-border bg-muted px-2.5 py-2 font-mono text-[11.5px] text-foreground"
        >
          {token}
        </code>
        <Button variant="secondary" size="icon" onClick={() => void copy()} aria-label="Copy token">
          {copied ? <Check className="size-4" /> : <Copy className="size-4" />}
        </Button>
      </div>
      <p className="text-[10.5px] leading-4 text-faint">
        {expiresAt ? `Expires ${formatDateTime(expiresAt)}` : 'Never expires'}
      </p>
      {onAdopt ? (
        <Button variant="primary" onClick={onAdopt}>
          Use this token
        </Button>
      ) : null}
    </div>
  );
}

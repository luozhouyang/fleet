import { LockKeyhole } from 'lucide-react';
import { useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useUnauthorized, useUnauthorizedReason } from '@/lib/api/auth';
import { setApiToken, useApiToken } from '@/lib/api/token';

/**
 * Blocks the app until a usable API token is available.
 *
 * The gate appears when no token is stored (including production builds with
 * no dev default) and whenever any REST call is rejected with HTTP 401. Saving
 * a token stores it, clears the rejection flag and mounts the app; the event
 * WebSocket reconnects automatically because it is mounted below the gate.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const token = useApiToken();
  const unauthorized = useUnauthorized();
  const unauthorizedReason = useUnauthorizedReason();
  const [draft, setDraft] = useState(token);

  // Keep the field in sync when the token is changed elsewhere (settings or
  // another tab) and the gate becomes visible.
  useEffect(() => {
    setDraft(token);
  }, [token]);

  if (token && !unauthorized) return <>{children}</>;

  const expired = unauthorizedReason === 'token_expired';
  const missing = !token && !expired;
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const value = draft.trim();
    if (!value) return;
    setApiToken(value);
    setDraft(value);
  };

  return (
    <div
      data-testid="auth-gate"
      className="flex min-h-dvh items-center justify-center bg-background px-6 py-12"
    >
      <div className="w-full max-w-[420px]">
        <div className="flex flex-col items-center text-center">
          <span className="flex size-11 items-center justify-center rounded-xl bg-primary-soft text-primary">
            <LockKeyhole className="size-5" aria-hidden="true" />
          </span>
          <h1 className="mt-4 font-display text-[22px] font-bold leading-7 tracking-[-0.02em] text-foreground">
            Fleet
          </h1>
          <p className="mt-1 text-[12.5px] leading-5 text-muted-foreground">
            {expired
              ? 'This API token has expired. Rotate it from another session, or paste a new token to continue.'
              : missing
                ? 'Enter an API token to connect to this admin.'
                : 'The API rejected the saved token. Enter a valid token to continue.'}
          </p>
        </div>

        <form
          onSubmit={submit}
          className="mt-6 flex flex-col gap-3 rounded-lg border border-border bg-card p-5 shadow-card"
        >
          <div className="flex flex-col gap-1.5">
            <label htmlFor="auth-gate-token" className="text-[11px] font-bold text-muted-foreground">
              API token
            </label>
            <Input
              id="auth-gate-token"
              type="password"
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              placeholder="Paste your bearer token"
              autoComplete="off"
              spellCheck={false}
              autoFocus
              className="font-mono text-[11.5px]"
            />
            <p className="text-[10.5px] leading-4 text-faint">
              Used for REST requests and the live event connection. It is stored in this browser
              only.
            </p>
          </div>
          <Button
            type="submit"
            variant="primary"
            disabled={!draft.trim()}
            className="w-full"
          >
            {missing ? 'Connect' : 'Try again'}
          </Button>
        </form>
      </div>
    </div>
  );
}

import { LockKeyhole } from 'lucide-react';
import { useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useUnauthorized, useUnauthorizedReason } from '@/lib/api/auth';
import { hasBrowserSession, upgradeToSession } from '@/lib/api/session';
import { clearApiToken, setApiToken, useApiToken } from '@/lib/api/token';

type Phase = 'checking' | 'ready' | 'gate';

/**
 * Blocks the app until it has a usable credential.
 *
 * A user bearer token is exchanged for an HttpOnly session cookie and then
 * removed from storage; a legacy bootstrap token cannot own a session and is
 * kept as a bearer credential. When no token is stored, an existing session
 * cookie is enough. Any later 401 brings the gate back.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const token = useApiToken();
  const unauthorized = useUnauthorized();
  const unauthorizedReason = useUnauthorizedReason();
  const [phase, setPhase] = useState<Phase>('checking');
  const [sessionReady, setSessionReady] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState(token);

  // Keep the field in sync when the token is changed elsewhere (settings or
  // another tab) and the gate becomes visible.
  useEffect(() => {
    setDraft(token);
  }, [token]);

  useEffect(() => {
    if (!unauthorized) return;
    setSessionReady(false);
    setPhase('gate');
  }, [unauthorized]);

  useEffect(() => {
    if (sessionReady || unauthorized) return;
    let cancelled = false;

    const boot = async () => {
      setPhase('checking');
      if (token) {
        const result = await upgradeToSession(token);
        if (cancelled) return;
        if (result === 'session') {
          // The cookie is now the credential; drop the long-lived token.
          setApiToken('');
          setSessionReady(true);
          setPhase('ready');
          return;
        }
        if (result === 'legacy_token' || result === 'unavailable') {
          setPhase('ready');
          return;
        }
        // The stored token is no longer valid; a live session may still exist.
        clearApiToken();
      }
      const alive = await hasBrowserSession();
      if (cancelled) return;
      setPhase(alive ? 'ready' : 'gate');
    };

    void boot();
    return () => {
      cancelled = true;
    };
  }, [token, sessionReady, unauthorized]);

  if (phase === 'ready') return <>{children}</>;

  if (phase === 'checking') {
    return (
      <div className="flex min-h-dvh items-center justify-center bg-background px-6">
        <p className="font-mono text-[10px] uppercase tracking-[0.14em] text-faint">
          Connecting to this admin…
        </p>
      </div>
    );
  }

  const expired = unauthorizedReason === 'token_expired';
  const missing = !token && !expired;
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const value = draft.trim();
    if (!value || submitting) return;
    setSubmitting(true);
    setError(null);
    const result = await upgradeToSession(value);
    setSubmitting(false);
    if (result === 'session') {
      setApiToken('');
      setSessionReady(true);
      setPhase('ready');
      return;
    }
    if (result === 'legacy_token' || result === 'unavailable') {
      setApiToken(value);
      setPhase('ready');
      return;
    }
    setError('The API rejected this token. Check that it has not expired.');
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
          onSubmit={(event) => void submit(event)}
          className="mt-6 flex flex-col gap-3 rounded-lg border border-border bg-card p-5 shadow-card"
        >
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="auth-gate-token"
              className="text-[11px] font-bold text-muted-foreground"
            >
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
              A user token becomes a browser session and is not stored. A legacy admin token stays a
              bearer credential.
            </p>
            {error ? <p className="text-[10.5px] leading-4 text-destructive">{error}</p> : null}
          </div>
          <Button
            type="submit"
            variant="primary"
            disabled={!draft.trim() || submitting}
            className="w-full"
          >
            {submitting ? 'Connecting…' : missing ? 'Connect' : 'Try again'}
          </Button>
        </form>
      </div>
    </div>
  );
}

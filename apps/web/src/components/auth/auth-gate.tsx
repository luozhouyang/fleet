import type { AuthOptions } from '@agent-fleet/protocol-types';
import { LockKeyhole } from 'lucide-react';
import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from 'react';
import { Button, buttonVariants } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useUnauthorized, useUnauthorizedReason } from '@/lib/api/auth';
import { apiClient } from '@/lib/api/client';
import { API_BASE_URL } from '@/lib/api/config';
import { hasBrowserSession, upgradeToSession } from '@/lib/api/session';
import { clearApiToken, setApiToken, useApiToken } from '@/lib/api/token';

type Phase = 'checking' | 'ready' | 'gate';

const AUTH_ERRORS: Record<string, string> = {
  provider_denied: 'Sign-in was cancelled at the provider.',
  state_invalid: 'The sign-in request expired. Try again.',
  exchange_failed: 'The provider rejected the sign-in. Try again.',
  email_unverified: 'The provider did not report a verified email address.',
  not_initialized:
    'This deployment has no administrator yet. Sign in with a bootstrap email or use a CLI token.',
  account_disabled: 'This account has been disabled.',
  provider_not_configured: 'That sign-in provider is not configured.',
  unknown_provider: 'That sign-in provider is not configured.',
  internal: 'Sign-in failed. Check the server logs.',
};

function providerLabel(provider: string): string {
  if (provider === 'github') return 'GitHub';
  if (provider === 'google') return 'Google';
  return provider.charAt(0).toUpperCase() + provider.slice(1);
}

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
  const [providers, setProviders] = useState<AuthOptions | null>(null);

  const authError = useMemo(
    () => new URLSearchParams(window.location.search).get('auth_error'),
    [],
  );
  const authErrorMessage = authError
    ? (AUTH_ERRORS[authError] ?? 'Sign-in failed. Try again.')
    : null;

  // The callback redirect carries the failure in the query string; show it
  // once, then remove it so a reload does not repeat the message.
  useEffect(() => {
    if (!authError) return;
    const url = new URL(window.location.href);
    url.searchParams.delete('auth_error');
    window.history.replaceState({}, '', url.toString());
  }, [authError]);

  useEffect(() => {
    let cancelled = false;
    apiClient
      .authProviders()
      .then((options) => {
        if (!cancelled) setProviders(options);
      })
      .catch(() => {
        // The token form remains available when the endpoint is unreachable.
      });
    return () => {
      cancelled = true;
    };
  }, []);

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
  const oauthProviders = providers?.providers ?? [];
  const hasOauth = oauthProviders.length > 0;
  // Once OAuth is configured the token form disappears. The one exception is
  // a deployment that still needs its first account and has no bootstrap
  // email: then a CLI token is the only way in.
  const bootstrapNeedsToken =
    providers?.bootstrap_required === true && !providers.bootstrap_email_configured;
  const showTokenForm = !hasOauth || bootstrapNeedsToken;
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
            {authErrorMessage ??
              (expired
                ? hasOauth
                  ? 'This API token has expired. Sign in again to continue.'
                  : 'This API token has expired. Rotate it from another session, or paste a new token to continue.'
                : missing
                  ? hasOauth
                    ? 'Sign in to Fleet.'
                    : 'Enter an API token to connect to this admin.'
                  : hasOauth
                    ? 'The API rejected the saved token. Sign in to continue.'
                    : 'The API rejected the saved token. Enter a valid token to continue.')}
          </p>
        </div>

        {hasOauth ? (
          <div className="mt-6 flex flex-col gap-2">
            {oauthProviders.map((provider) => (
              <a
                key={provider}
                href={`${API_BASE_URL}/auth/login/${encodeURIComponent(provider)}`}
                data-testid={`oauth-login-${provider}`}
                className={buttonVariants({ variant: 'primary', size: 'lg' })}
              >
                Continue with {providerLabel(provider)}
              </a>
            ))}
            {providers?.bootstrap_required ? (
              <p className="mt-1 text-center text-[10.5px] leading-4 text-faint">
                {providers.bootstrap_email_configured
                  ? 'The first sign-in must use a configured bootstrap administrator email.'
                  : 'No bootstrap administrator email is configured; use a CLI token to create the first account.'}
              </p>
            ) : null}
          </div>
        ) : null}

        {hasOauth && showTokenForm ? (
          <div className="my-5 flex items-center gap-3 font-mono text-[9px] uppercase tracking-[0.14em] text-faint">
            <span className="h-px flex-1 bg-border" />
            or use an API token
            <span className="h-px flex-1 bg-border" />
          </div>
        ) : null}

        {showTokenForm ? (
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
                A user token becomes a browser session and is not stored. A legacy admin token stays
                a bearer credential.
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
        ) : null}
      </div>
    </div>
  );
}

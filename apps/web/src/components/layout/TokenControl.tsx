import { Button, Dialog, Field, Input, StatusDot } from '@agent-fleet/ui';
import { useEffect, useState } from 'react';
import { setApiToken, useApiToken } from '@/lib/api/token';

function maskToken(token: string): string {
  if (token.length <= 6) return '••••••';
  return `${token.slice(0, 3)}••${token.slice(-3)}`;
}

export function TokenControl() {
  const token = useApiToken();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(token);
  const [showToken, setShowToken] = useState(false);

  useEffect(() => {
    if (open) {
      setDraft(token);
      setShowToken(false);
    }
  }, [open, token]);

  const save = () => {
    setApiToken(draft);
    setOpen(false);
  };

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="group inline-flex h-9 items-center gap-2 rounded-md border border-slate-800 bg-slate-900/70 px-2.5 text-xs text-slate-400 transition-colors hover:border-slate-700 hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/60"
        aria-label="Edit API token"
      >
        <StatusDot tone="green" />
        <span className="hidden sm:inline">Token</span>
        <span className="font-mono text-[11px] text-slate-500 group-hover:text-slate-300">
          {maskToken(token)}
        </span>
      </button>

      <Dialog
        open={open}
        onClose={() => setOpen(false)}
        title="API access token"
        description="Sent as a Bearer token with REST requests and as the token query parameter on the event WebSocket."
        footer={
          <>
            <Button variant="ghost" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={save} disabled={!draft.trim()}>
              Save token
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <Field
            label="Bearer token"
            htmlFor="api-token"
            hint="Stored only in this browser's localStorage. The default development token is dev-token."
          >
            <div className="flex gap-2">
              <Input
                id="api-token"
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                type={showToken ? 'text' : 'password'}
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
                placeholder="dev-token"
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && draft.trim()) save();
                }}
              />
              <Button
                variant="secondary"
                onClick={() => setShowToken((value) => !value)}
                className="shrink-0"
              >
                {showToken ? 'Hide' : 'Show'}
              </Button>
            </div>
          </Field>
          <p className="rounded-md border border-slate-800 bg-slate-950/50 px-3 py-2 text-xs leading-5 text-slate-500">
            Changing the token reconnects the live event stream immediately.
          </p>
        </div>
      </Dialog>
    </>
  );
}

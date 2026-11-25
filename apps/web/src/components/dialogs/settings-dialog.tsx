import { Eye, EyeOff } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { MyIdentitiesSection } from '@/features/users/MyIdentitiesSection';
import { MyTokenSection } from '@/features/users/MyTokenSection';
import { logoutBrowserSession } from '@/lib/api/session';
import { getGitWriteToken, setApiToken, setGitWriteToken, useApiToken } from '@/lib/api/token';

function TokenField({
  id,
  label,
  value,
  onChange,
  revealed,
  onToggleReveal,
  placeholder,
  hint,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  revealed: boolean;
  onToggleReveal: () => void;
  placeholder?: string;
  hint?: string;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-[11px] font-bold text-muted-foreground">
        {label}
      </label>
      <div className="flex items-center gap-1.5">
        <Input
          id={id}
          type={revealed ? 'text' : 'password'}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder={placeholder}
          className="font-mono text-[11.5px]"
          autoComplete="off"
          spellCheck={false}
        />
        <Button
          variant="ghost"
          size="icon"
          aria-label={`${revealed ? 'Hide' : 'Show'} ${label}`}
          title={`${revealed ? 'Hide' : 'Show'} ${label}`}
          onClick={onToggleReveal}
          className="shrink-0"
        >
          {revealed ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
        </Button>
      </div>
      {hint ? <p className="text-[10.5px] leading-4 text-faint">{hint}</p> : null}
    </div>
  );
}

export function SettingsDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const currentToken = useApiToken();
  const [apiToken, setApiTokenValue] = useState(currentToken);
  const [gitToken, setGitTokenValue] = useState(getGitWriteToken());
  const [revealApi, setRevealApi] = useState(false);
  const [revealGit, setRevealGit] = useState(false);
  const [signingOut, setSigningOut] = useState(false);

  const signOut = async () => {
    if (signingOut) return;
    setSigningOut(true);
    await logoutBrowserSession();
    setApiToken('');
    window.location.assign('/');
  };

  // Keep the field in sync when a rotation from the "My token" section adopts
  // a new credential while this dialog is open.
  useEffect(() => {
    setApiTokenValue(currentToken);
  }, [currentToken]);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (next) {
          setApiTokenValue(currentToken);
          setGitTokenValue(getGitWriteToken());
          setRevealApi(false);
          setRevealGit(false);
        }
        onOpenChange(next);
      }}
    >
      <DialogContent className="max-w-[520px]">
        <DialogHeader
          title="Connection settings"
          description="Tokens are stored in this browser only."
          onClose={() => onOpenChange(false)}
        />
        <form
          className="mt-5 flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            setApiToken(apiToken);
            setGitWriteToken(gitToken);
            onOpenChange(false);
          }}
        >
          <TokenField
            id="settings-api-token"
            label="API token"
            value={apiToken}
            onChange={setApiTokenValue}
            revealed={revealApi}
            onToggleReveal={() => setRevealApi((value) => !value)}
            hint="A user token becomes a browser session; the live event socket uses a short-lived ticket."
          />
          <TokenField
            id="settings-git-token"
            label="Git write token"
            value={gitToken}
            onChange={setGitTokenValue}
            revealed={revealGit}
            onToggleReveal={() => setRevealGit((value) => !value)}
            placeholder="Optional; required for stage, commit and checkout"
          />
          <div className="mt-1 flex items-center justify-between gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => void signOut()}
              disabled={signingOut}
            >
              {signingOut ? 'Signing out…' : 'Sign out'}
            </Button>
            <div className="flex items-center gap-2">
              <Button variant="secondary" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" variant="primary">
                Save
              </Button>
            </div>
          </div>
        </form>
        <MyTokenSection />
        <MyIdentitiesSection />
      </DialogContent>
    </Dialog>
  );
}

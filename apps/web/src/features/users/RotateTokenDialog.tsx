import type { RotatedUserToken } from '@agent-fleet/protocol-types';
import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Select } from '@/components/ui/select';
import { setApiToken } from '@/lib/api/token';
import { getErrorMessage } from '@/lib/utils/format';
import { useRotateUserToken, userKeys } from './hooks';
import { TokenReveal } from './TokenReveal';
import { EXPIRY_OPTIONS, expiryChoiceToDays } from './user-utils';

export function RotateTokenDialog({
  userId,
  userLabel,
  self,
  onClose,
}: {
  userId: string;
  userLabel: string;
  /** True when the token belongs to the caller, so the new value is adopted. */
  self: boolean;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const mutation = useRotateUserToken();
  const [expiry, setExpiry] = useState('default');
  const [rotated, setRotated] = useState<RotatedUserToken | null>(null);

  const adopt = (result: RotatedUserToken) => {
    setApiToken(result.token);
    void queryClient.invalidateQueries({ queryKey: userKeys.me() });
  };

  const submit = () => {
    if (mutation.isPending) return;
    mutation.mutate(
      {
        userId,
        expiresInDays: expiryChoiceToDays(expiry),
      },
      {
        onSuccess: (result) => {
          // Rotating the caller's own token invalidates it immediately, so the
          // browser must switch to the new value before any other request runs.
          if (self) adopt(result);
          else {
            void queryClient.invalidateQueries({ queryKey: userKeys.usersRoot() });
            void queryClient.invalidateQueries({ queryKey: userKeys.membersRoot() });
          }
          setRotated(result);
        },
      },
    );
  };

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <DialogContent className="max-w-[520px]">
        <DialogHeader
          title={rotated ? 'New token' : 'Rotate token'}
          description={
            rotated
              ? 'The previous token no longer works. This value is shown once.'
              : `The current token for ${userLabel} stops working immediately.`
          }
          onClose={onClose}
        />
        {rotated ? (
          <>
            <TokenReveal
              token={rotated.token}
              expiresAt={rotated.token_expires_at}
              onAdopt={
                self
                  ? () => {
                      adopt(rotated);
                      onClose();
                    }
                  : undefined
              }
            />
            <div className="mt-5 flex items-center justify-end">
              <Button variant="primary" onClick={onClose}>
                Done
              </Button>
            </div>
          </>
        ) : (
          <form
            className="mt-5 flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              submit();
            }}
          >
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="rotate-token-expiry"
                className="text-[11px] font-bold text-muted-foreground"
              >
                New token expiry
              </label>
              <Select
                id="rotate-token-expiry"
                value={expiry}
                onValueChange={setExpiry}
                options={EXPIRY_OPTIONS}
                disabled={mutation.isPending}
              />
            </div>

            {mutation.error ? (
              <p className="rounded-md border border-destructive/20 bg-destructive-soft px-3 py-2 text-[11.5px] text-destructive">
                {getErrorMessage(mutation.error)}
              </p>
            ) : null}

            <div className="mt-1 flex items-center justify-end gap-2">
              <Button variant="secondary" onClick={onClose} disabled={mutation.isPending}>
                Cancel
              </Button>
              <Button type="submit" variant="primary" disabled={mutation.isPending}>
                {mutation.isPending ? 'Rotating…' : 'Rotate token'}
              </Button>
            </div>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

import type { CreatedUser, Organization, UserRole } from '@agent-fleet/protocol-types';
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Select } from '@/components/ui/select';
import { getErrorMessage } from '@/lib/utils/format';
import { useCreateUser } from './hooks';
import { TokenReveal } from './TokenReveal';
import { EXPIRY_OPTIONS, expiryChoiceToDays, ROLE_OPTIONS } from './user-utils';

export function CreateUserDialog({
  organizations,
  onClose,
}: {
  organizations: Organization[];
  onClose: () => void;
}) {
  const mutation = useCreateUser();
  const [name, setName] = useState('');
  const [orgId, setOrgId] = useState(organizations[0]?.id ?? '');
  const [role, setRole] = useState<UserRole>('member');
  const [expiry, setExpiry] = useState('default');
  const [created, setCreated] = useState<CreatedUser | null>(null);

  useEffect(() => {
    if (!orgId && organizations[0]) setOrgId(organizations[0].id);
  }, [orgId, organizations]);

  const canSubmit = Boolean(name.trim() && orgId) && !mutation.isPending;

  const submit = () => {
    if (!canSubmit) return;
    const days = expiryChoiceToDays(expiry);
    mutation.mutate(
      {
        org_id: orgId,
        name: name.trim(),
        role,
        ...(days === undefined ? {} : { token_expires_in_days: days }),
      },
      { onSuccess: setCreated },
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
          title={created ? `Token for ${created.user.name}` : 'Create user'}
          description={
            created
              ? 'Share this token with the user; it will not be shown again.'
              : 'The new user receives a bearer token scoped to one organization.'
          }
          onClose={onClose}
        />
        {created ? (
          <>
            <TokenReveal token={created.token} expiresAt={created.user.token_expires_at} />
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
                htmlFor="create-user-name"
                className="text-[11px] font-bold text-muted-foreground"
              >
                Name
              </label>
              <Input
                id="create-user-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder="Ada Lovelace"
                disabled={mutation.isPending}
                required
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="create-user-org"
                className="text-[11px] font-bold text-muted-foreground"
              >
                Organization
              </label>
              <Select
                id="create-user-org"
                value={orgId}
                onValueChange={setOrgId}
                options={organizations.map((organization) => ({
                  value: organization.id,
                  label: organization.name,
                  description: organization.id,
                }))}
                placeholder="Select an organization"
                disabled={mutation.isPending || organizations.length === 0}
              />
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <label
                  htmlFor="create-user-role"
                  className="text-[11px] font-bold text-muted-foreground"
                >
                  Role
                </label>
                <Select
                  id="create-user-role"
                  value={role}
                  onValueChange={(value) => setRole(value as UserRole)}
                  options={ROLE_OPTIONS}
                  disabled={mutation.isPending}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <label
                  htmlFor="create-user-expiry"
                  className="text-[11px] font-bold text-muted-foreground"
                >
                  Token expiry
                </label>
                <Select
                  id="create-user-expiry"
                  value={expiry}
                  onValueChange={setExpiry}
                  options={EXPIRY_OPTIONS}
                  disabled={mutation.isPending}
                />
              </div>
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
              <Button type="submit" variant="primary" disabled={!canSubmit}>
                {mutation.isPending ? 'Creating…' : 'Create user'}
              </Button>
            </div>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

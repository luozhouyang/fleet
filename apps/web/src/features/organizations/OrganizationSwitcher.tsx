import { UserRound } from 'lucide-react';
import { Select } from '@/components/ui/select';
import { setActiveOrgId, useActiveOrgId } from '@/lib/api/organization';
import { useMe, useOrganizations } from '@/features/users/hooks';
import { ROLE_LABELS } from '@/features/users/user-utils';

/**
 * Active-organization selector.
 *
 * Team organizations are always listed. The personal space is hidden while a
 * team exists, matching the product rule that a shared organization is the
 * default; it stays reachable through the adjacent personal-space button.
 */
export function OrganizationSwitcher() {
  const meQuery = useMe();
  const organizationsQuery = useOrganizations();
  const organizations = organizationsQuery.data ?? [];
  const storedOrgId = useActiveOrgId();

  // The legacy admin token has no membership, so `/me` reports no active
  // organization; its selection is client-side only.
  const activeOrgId = storedOrgId || meQuery.data?.org_id || '';
  const teams = organizations.filter((summary) => summary.organization.kind === 'team');
  const personal = organizations.find((summary) => summary.organization.kind === 'personal');
  const personalIsActive = personal !== undefined && personal.organization.id === activeOrgId;
  const personalIsHidden = personal !== undefined && teams.length > 0 && !personalIsActive;

  const visible = [...teams, ...(personal && !personalIsHidden ? [personal] : [])];
  const options = visible.map((summary) => ({
    value: summary.organization.id,
    label: summary.organization.name,
    description: ROLE_LABELS[summary.role],
  }));

  if (options.length === 0) return null;

  return (
    <span className="flex items-center gap-1">
      <Select
        aria-label="Active organization"
        value={activeOrgId}
        onValueChange={setActiveOrgId}
        options={options}
        placeholder="Organization"
        disabled={options.length === 1}
        className="h-8 w-[158px] text-[11.5px]"
      />
      {personalIsHidden && personal ? (
        <button
          type="button"
          onClick={() => setActiveOrgId(personal.organization.id)}
          title={`Switch to ${personal.organization.name}`}
          aria-label="Switch to personal space"
          className="flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <UserRound className="size-3.5" />
        </button>
      ) : null}
    </span>
  );
}

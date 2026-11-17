/**
 * Human-readable machine label.
 *
 * Machine ids are opaque and can look like credentials; never use one as a
 * display fallback. Older rows may still carry the id in `name`, so prefer the
 * hostname in that case.
 */
export function machineDisplayName(
  machine: { id?: string; name?: string | null; hostname?: string | null } | null | undefined,
): string {
  const name = machine?.name?.trim();
  if (name && name !== machine?.id) return name;
  const hostname = machine?.hostname?.trim();
  if (hostname && hostname !== 'unknown' && hostname !== machine?.id) return hostname;
  return 'Unknown machine';
}

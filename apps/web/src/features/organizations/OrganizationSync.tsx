import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { eventClient } from '@/lib/api/client';
import { useActiveOrgId } from '@/lib/api/organization';

/**
 * Reset organization-scoped state when the selection changes.
 *
 * Every cached query was answered under the previous organization, and the
 * event socket's ticket is bound to it, so both are discarded. This also runs
 * for a change made in another tab through the storage event.
 */
export function OrganizationSync() {
  const orgId = useActiveOrgId();
  const queryClient = useQueryClient();
  const previous = useRef(orgId);

  useEffect(() => {
    if (previous.current === orgId) return;
    previous.current = orgId;
    queryClient.clear();
    void eventClient.reconnect();
  }, [orgId, queryClient]);

  return null;
}

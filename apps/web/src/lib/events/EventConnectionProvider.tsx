import { useEffect, type ReactNode } from 'react';
import { eventClient } from '@/lib/api/client';
import { useApiToken } from '@/lib/api/token';

export function EventConnectionProvider({ children }: { children: ReactNode }) {
  const token = useApiToken();

  useEffect(() => {
    void eventClient.connect();
    return () => {
      eventClient.disconnect();
    };
  }, [token]);

  return <>{children}</>;
}

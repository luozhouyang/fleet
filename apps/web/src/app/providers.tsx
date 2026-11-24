import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import { AuthGate } from '@/components/auth/auth-gate';
import { OrganizationSync } from '@/features/organizations/OrganizationSync';
import { EventBridge } from '@/lib/events/EventBridge';
import { EventConnectionProvider } from '@/lib/events/EventConnectionProvider';
import { router } from './router';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 15_000,
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

export function AppProviders() {
  return (
    <QueryClientProvider client={queryClient}>
      <AuthGate>
        <EventConnectionProvider>
          <OrganizationSync />
          <EventBridge />
          <RouterProvider router={router} />
        </EventConnectionProvider>
      </AuthGate>
    </QueryClientProvider>
  );
}

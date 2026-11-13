import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
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
      <EventConnectionProvider>
        <EventBridge />
        <RouterProvider router={router} />
      </EventConnectionProvider>
    </QueryClientProvider>
  );
}

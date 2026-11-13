import {
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
} from '@tanstack/react-router';
import { AppShell } from '@/components/layout/AppShell';
import { NotFoundPage, RouteError } from '@/components/feedback/RouteFallbacks';
import { MachineDetailPage } from '@/features/machines/MachineDetailPage';
import { MachinesPage } from '@/features/machines/MachinesPage';
import { SessionPage } from '@/features/sessions/SessionPage';
import { SessionsPage } from '@/features/sessions/SessionsPage';
import { WorkspacePage } from '@/features/workspaces/WorkspacePage';

const rootRoute = createRootRoute({
  component: AppShell,
  notFoundComponent: NotFoundPage,
  errorComponent: RouteError,
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  beforeLoad: () => {
    throw redirect({ to: '/machines' });
  },
});

const machinesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/machines',
  component: MachinesPage,
});

const machineDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/machines/$machineId',
  component: MachineDetailPage,
});

const workspaceRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/workspaces/$workspaceId',
  component: WorkspacePage,
});

const sessionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/sessions',
  component: SessionsPage,
});

const sessionDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/sessions/$sessionId',
  component: SessionPage,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  machinesRoute,
  machineDetailRoute,
  workspaceRoute,
  sessionsRoute,
  sessionDetailRoute,
]);

export const router = createRouter({
  routeTree,
  defaultPreload: 'intent',
  scrollRestoration: true,
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}

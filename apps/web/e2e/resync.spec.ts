import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const USER_TOKEN = 'flu_test_user';

const ORG = {
  id: 'org_default',
  name: 'Default',
  kind: 'team' as const,
  owner_user_id: 'user_ada',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const ME = {
  kind: 'user',
  user: {
    id: 'user_ada',
    email: 'ada@example.com',
    name: 'Ada Admin',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    token_issued_at: '2026-01-01T00:00:00Z',
    token_expires_at: null,
  },
  org_id: ORG.id,
  role: 'admin',
};

interface FakeSocket {
  readyState: number;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: string }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  onclose: ((event: unknown) => void) | null;
  close: (code?: number, reason?: string) => void;
}

declare global {
  interface Window {
    __fleetSockets: FakeSocket[];
  }
}

/**
 * Replace the browser WebSocket with a stub that opens immediately and only
 * speaks when the test tells it to. This models a half-open connection: the
 * server has stopped delivering frames but the browser never fires `close`,
 * which is what the heartbeat watchdog exists for.
 */
async function installFakeWebSocket(page: Page) {
  await page.addInitScript(() => {
    class TestWebSocket {
      static readonly CONNECTING = 0;
      static readonly OPEN = 1;
      static readonly CLOSING = 2;
      static readonly CLOSED = 3;

      readyState = TestWebSocket.CONNECTING;
      onopen: ((event: unknown) => void) | null = null;
      onmessage: ((event: { data: string }) => void) | null = null;
      onerror: ((event: unknown) => void) | null = null;
      onclose: ((event: unknown) => void) | null = null;

      constructor() {
        window.__fleetSockets.push(this);
        queueMicrotask(() => {
          this.readyState = TestWebSocket.OPEN;
          this.onopen?.({});
        });
      }

      send() {}

      close() {
        this.readyState = TestWebSocket.CLOSED;
        const handler = this.onclose;
        this.onclose = null;
        queueMicrotask(() => handler?.({}));
      }
    }

    window.__fleetSockets = [];
    Object.defineProperty(window, 'WebSocket', {
      configurable: true,
      writable: true,
      value: TestWebSocket,
    });
  });
}

async function mockApi(page: Page, onMachinesRequest: () => void) {
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    if (path === '/api/v1/auth/session' && method === 'POST') {
      await route.fulfill({
        status: 200,
        headers: {
          'set-cookie': 'fleet_session=test-session; Path=/; HttpOnly; SameSite=Lax',
        },
        json: ME,
      });
      return;
    }
    if (path === '/api/v1/me') {
      await route.fulfill({ json: ME });
      return;
    }
    if (path === '/api/v1/orgs') {
      await route.fulfill({ json: [{ organization: ORG, role: 'owner' }] });
      return;
    }
    if (path === '/api/v1/realtime/ticket' && method === 'POST') {
      await route.fulfill({
        json: { ticket: 'test-ticket', org_id: ORG.id, expires_in: 60 },
      });
      return;
    }
    if (path === '/api/v1/machines') {
      onMachinesRequest();
      await route.fulfill({ json: [] });
      return;
    }
    if (path === '/api/v1/sessions') {
      await route.fulfill({ json: [] });
      return;
    }
    await route.fulfill({
      status: 404,
      json: { code: 'not_found', message: `unmocked request: ${method} ${path}` },
    });
  });
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(
    ([key, token]) => window.localStorage.setItem(key, token),
    [TOKEN_STORAGE_KEY, USER_TOKEN],
  );
});

test('reconnects after a silent socket and resyncs when connectivity returns', async ({ page }) => {
  await page.clock.install();
  await installFakeWebSocket(page);

  let machineRequests = 0;
  await mockApi(page, () => {
    machineRequests += 1;
  });

  await page.goto('/sessions');
  const live = page.locator('header').getByText('Live', { exact: true });
  await expect(live).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.__fleetSockets.length)).toBe(1);

  const beforeSilence = machineRequests;

  // Sixty seconds of silence: the 45-second watchdog drops the half-open
  // socket and the client reconnects on its own, then refetches the live views.
  await page.clock.runFor(60_000);
  await expect.poll(() => page.evaluate(() => window.__fleetSockets.length)).toBeGreaterThan(1);
  await expect(live).toBeVisible();
  await expect.poll(() => machineRequests).toBeGreaterThan(beforeSilence);

  // Returning online refetches without tearing down a healthy socket.
  const socketsAfterReconnect = await page.evaluate(() => window.__fleetSockets.length);
  const beforeOnline = machineRequests;
  await page.evaluate(() => window.dispatchEvent(new Event('online')));
  await expect.poll(() => machineRequests).toBeGreaterThan(beforeOnline);
  expect(await page.evaluate(() => window.__fleetSockets.length)).toBe(socketsAfterReconnect);

  // Returning to a visible tab does the same.
  const beforeVisible = machineRequests;
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => 'visible',
    });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect.poll(() => machineRequests).toBeGreaterThan(beforeVisible);
});

import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const TOKEN = 'fla_test_admin';

const ORG = {
  id: 'org_default',
  name: 'Default',
  kind: 'team' as const,
  owner_user_id: 'user_admin',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const ME = {
  kind: 'user',
  user: {
    id: 'user_admin',
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

const MACHINE = {
  id: 'm_1',
  name: 'build-01',
  hostname: 'build-01.local',
  os: 'linux',
  arch: 'x86_64',
  daemon_version: '0.2.2',
  protocol_version: 1,
  status: 'online',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  connected_at: '2026-01-01T00:00:00Z',
  last_seen_at: new Date().toISOString(),
  workspace_count: 0,
  providers: [],
  workspaces: [],
};

/**
 * Mock the admin API for the machine detail page and record rotations.
 *
 * The Rust admin test covers the two-phase confirmation; this spec verifies
 * that the dashboard action is offered to an admin and issues the request.
 */
async function mockAdminApi(page: Page, rotations: string[]) {
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    if (path === '/api/v1/auth/session' && method === 'POST') {
      await route.fulfill({
        status: 403,
        json: { code: 'forbidden', message: 'the bootstrap admin token cannot create a session' },
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
      await route.fulfill({ json: { ticket: 'test-ticket', org_id: ORG.id, expires_in: 60 } });
      return;
    }
    if (path === '/api/v1/machines/m_1/rotate-token' && method === 'POST') {
      rotations.push(path);
      await route.fulfill({
        json: { machine_id: 'm_1', rotated_at: new Date().toISOString() },
      });
      return;
    }
    if (path === '/api/v1/machines/m_1') {
      await route.fulfill({ json: MACHINE });
      return;
    }
    if (path === '/api/v1/machines/m_1/providers' || path === '/api/v1/drivers') {
      await route.fulfill({ json: [] });
      return;
    }
    if (
      path === '/api/v1/machines' ||
      path === '/api/v1/workspaces' ||
      path === '/api/v1/sessions'
    ) {
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
    [TOKEN_STORAGE_KEY, TOKEN],
  );
});

test('an admin rotates a machine token from the detail page', async ({ page }) => {
  const rotations: string[] = [];
  await mockAdminApi(page, rotations);

  await page.goto('/machines/m_1');
  await expect(page.getByRole('heading', { name: 'build-01' })).toBeVisible();

  await page.getByRole('button', { name: 'Rotate machine token' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('Rotate machine token?');
  await dialog.getByRole('button', { name: 'Rotate token' }).click();

  await expect(dialog).toBeHidden();
  expect(rotations).toEqual(['/api/v1/machines/m_1/rotate-token']);
});

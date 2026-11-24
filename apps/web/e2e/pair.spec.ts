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

/**
 * Drive the pairing dialog with a mocked admin API. The real code creation is
 * covered by the Rust admin tests; this verifies the browser flow and that the
 * command shown to the operator redeems the code.
 */
async function mockAdminApi(page: Page, bodies: Record<string, unknown>[]) {
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
      await route.fulfill({
        json: { ticket: 'test-ticket', org_id: ORG.id, expires_in: 60 },
      });
      return;
    }
    if (path === '/api/v1/pairing/codes' && method === 'POST') {
      bodies.push(request.postDataJSON() as Record<string, unknown>);
      await route.fulfill({
        json: {
          id: 'pair_1',
          code: 'ABCD-EFGH-JKMN',
          expires_at: new Date(Date.now() + 600_000).toISOString(),
          org_id: ORG.id,
        },
      });
      return;
    }
    if (path === '/api/v1/machines' || path === '/api/v1/sessions') {
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

test('an admin creates a pairing code and sees the daemon command', async ({ page }) => {
  const bodies: Record<string, unknown>[] = [];
  await mockAdminApi(page, bodies);

  await page.goto('/machines');
  await expect(page.getByRole('heading', { name: 'Machines' })).toBeVisible();

  await page.getByRole('button', { name: 'Pair machine' }).first().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await dialog.getByLabel('Machine name (optional)').fill('build-01');
  await dialog.getByRole('button', { name: 'Create code' }).click();

  await expect(dialog.getByTestId('pairing-code')).toHaveText('ABCD-EFGH-JKMN');
  // The dialog derives the daemon URL from the page origin, which the Rust
  // harness sets to a dynamically allocated port.
  const wsOrigin = new URL(page.url()).origin.replace(/^http/, 'ws');
  await expect(dialog.getByTestId('pairing-command')).toContainText(
    `fleet daemon pair --server ${wsOrigin}/ws/machine --code ABCD-EFGH-JKMN`,
  );
  expect(bodies).toEqual([{ org_id: ORG.id, machine_name: 'build-01' }]);

  await dialog.getByRole('button', { name: 'Done' }).click();
  await expect(dialog).toBeHidden();
});

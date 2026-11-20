import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const TOKEN = 'fla_test_admin';

interface MockUser {
  id: string;
  org_id: string;
  name: string;
  role: 'admin' | 'member' | 'viewer';
  created_at: string;
  updated_at: string;
  token_issued_at: string;
  token_expires_at: string | null;
}

const ORG = { id: 'org_default', name: 'Default', created_at: '2026-01-01T00:00:00Z' };

function timestamp(daysFromNow: number): string {
  return new Date(Date.now() + daysFromNow * 86_400_000).toISOString();
}

function adminUser(): MockUser {
  return {
    id: 'user_admin',
    org_id: ORG.id,
    name: 'Ada Admin',
    role: 'admin',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    token_issued_at: timestamp(-23),
    token_expires_at: timestamp(7),
  };
}

/**
 * Drive the Users page with a stateful mock of the admin API.
 *
 * The real routes are covered by the Rust admin tests; this scenario verifies
 * the browser flow: the admin-only nav entry, create with a short expiry, the
 * one-time token reveal, rotation, and deletion.
 */
async function mockAdminApi(page: Page) {
  const users: MockUser[] = [adminUser()];
  let sequence = 0;

  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();

    if (path === '/api/v1/me') {
      await route.fulfill({
        json: { kind: 'user', user: users[0], org_id: ORG.id, role: 'admin' },
      });
      return;
    }
    if (path === '/api/v1/orgs') {
      await route.fulfill({ json: [ORG] });
      return;
    }
    if (path === '/api/v1/users' && method === 'GET') {
      await route.fulfill({ json: users });
      return;
    }
    if (path === '/api/v1/users' && method === 'POST') {
      sequence += 1;
      const body = request.postDataJSON() as {
        org_id: string;
        name: string;
        role: MockUser['role'];
        token_expires_in_days?: number;
      };
      const days = body.token_expires_in_days ?? 30;
      const created: MockUser = {
        id: `user_created_${sequence}`,
        org_id: body.org_id,
        name: body.name,
        role: body.role,
        created_at: timestamp(0),
        updated_at: timestamp(0),
        token_issued_at: timestamp(0),
        token_expires_at: days === 0 ? null : timestamp(days),
      };
      users.push(created);
      await route.fulfill({
        json: { user: created, token: `flu_created_${sequence}` },
      });
      return;
    }

    const rotate = path.match(/^\/api\/v1\/users\/([^/]+)\/rotate-token$/);
    if (rotate && method === 'POST') {
      const user = users.find((candidate) => candidate.id === rotate[1]);
      if (!user) {
        await route.fulfill({ status: 404, json: { code: 'not_found', message: 'no user' } });
        return;
      }
      sequence += 1;
      const days = Number(url.searchParams.get('expires_in_days') ?? 30);
      user.token_issued_at = timestamp(0);
      user.token_expires_at = days === 0 ? null : timestamp(days);
      await route.fulfill({
        json: {
          user_id: user.id,
          token: `flu_rotated_${sequence}`,
          token_expires_at: user.token_expires_at,
        },
      });
      return;
    }

    const remove = path.match(/^\/api\/v1\/users\/([^/]+)$/);
    if (remove && method === 'DELETE') {
      const index = users.findIndex((candidate) => candidate.id === remove[1]);
      if (index >= 0) users.splice(index, 1);
      await route.fulfill({ status: 204, body: '' });
      return;
    }

    if (path === '/api/v1/sessions' || path === '/api/v1/machines') {
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

test('admin creates, rotates and deletes a user with a one-time token reveal', async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto('/users');

  await expect(page.getByRole('heading', { name: 'Users' })).toBeVisible();
  await expect(page.getByTestId('user-row')).toHaveCount(1);
  await expect(page.getByRole('link', { name: 'Users' })).toBeVisible();

  // Create with a short expiry.
  await page.getByRole('button', { name: 'Create user' }).click();
  const createDialog = page.getByRole('dialog');
  await createDialog.getByLabel('Name').fill('Grace Hopper');
  await createDialog.getByLabel('Organization').click();
  await page.getByRole('option', { name: /Default/ }).click();
  await createDialog.getByLabel('Role').click();
  await page.getByRole('option', { name: 'Member' }).click();
  await createDialog.getByLabel('Token expiry').click();
  await page.getByRole('option', { name: '7 days' }).click();
  await createDialog.getByRole('button', { name: 'Create user' }).click();

  const createToken = createDialog.getByTestId('revealed-token');
  await expect(createToken).toHaveText('flu_created_1');
  await createDialog.getByRole('button', { name: 'Done' }).click();

  const graceRow = page.getByTestId('user-row').filter({ hasText: 'Grace Hopper' });
  await expect(graceRow).toContainText('in 7 days');
  await expect(page.getByTestId('user-row')).toHaveCount(2);

  // Rotate: the new token is revealed once and the expiry is refreshed.
  await graceRow.getByRole('button', { name: 'Rotate token for Grace Hopper' }).click();
  const rotateDialog = page.getByRole('dialog');
  await expect(rotateDialog).toContainText('stops working immediately');
  await rotateDialog.getByRole('button', { name: 'Rotate token' }).click();
  await expect(rotateDialog.getByTestId('revealed-token')).toHaveText('flu_rotated_2');
  await rotateDialog.getByRole('button', { name: 'Done' }).click();
  await expect(graceRow).toContainText('in 30 days');

  // Delete removes the row after confirmation.
  await graceRow.getByRole('button', { name: 'Delete Grace Hopper' }).click();
  const confirm = page.getByRole('dialog');
  await expect(confirm).toContainText('Delete Grace Hopper?');
  await confirm.getByRole('button', { name: 'Delete user' }).click();
  await expect(page.getByTestId('user-row')).toHaveCount(1);
  await expect(page.getByTestId('user-row').filter({ hasText: 'Grace Hopper' })).toHaveCount(0);
});

import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const TOKEN = 'fla_test_admin';

type Role = 'owner' | 'admin' | 'member' | 'viewer';

interface MockUser {
  id: string;
  email: string;
  name: string;
  created_at: string;
  updated_at: string;
  token_issued_at: string;
  token_expires_at: string | null;
}

interface MockMember {
  user_id: string;
  email: string;
  name: string;
  avatar_url: null;
  role: Role;
  created_at: string;
  updated_at: string;
}

const ORG = {
  id: 'org_default',
  name: 'Default',
  kind: 'team' as const,
  owner_user_id: 'user_admin',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

function timestamp(daysFromNow: number): string {
  return new Date(Date.now() + daysFromNow * 86_400_000).toISOString();
}

function adminUser(): MockUser {
  return {
    id: 'user_admin',
    email: 'ada@example.com',
    name: 'Ada Admin',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    token_issued_at: timestamp(-23),
    token_expires_at: timestamp(7),
  };
}

function memberOf(user: MockUser, role: Role): MockMember {
  return {
    user_id: user.id,
    email: user.email,
    name: user.name,
    avatar_url: null,
    role,
    created_at: user.created_at,
    updated_at: user.updated_at,
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
  const members: MockMember[] = [memberOf(users[0], 'admin')];
  let sequence = 0;

  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();

    if (path === '/api/v1/auth/session' && method === 'POST') {
      // The fixture uses the legacy admin token, which cannot own a session.
      await route.fulfill({
        status: 403,
        json: {
          code: 'forbidden',
          message: 'the bootstrap admin token cannot create a browser session',
        },
      });
      return;
    }
    if (path === '/api/v1/me') {
      await route.fulfill({
        json: { kind: 'user', user: users[0], org_id: ORG.id, role: 'admin' },
      });
      return;
    }
    if (path === '/api/v1/orgs') {
      await route.fulfill({
        json: [{ organization: ORG, role: 'owner' }],
      });
      return;
    }
    if (path === `/api/v1/orgs/${ORG.id}/members` && method === 'GET') {
      await route.fulfill({ json: members });
      return;
    }
    if (path === '/api/v1/realtime/ticket' && method === 'POST') {
      await route.fulfill({
        json: { ticket: 'test-ticket', org_id: ORG.id, expires_in: 60 },
      });
      return;
    }
    if (path === '/api/v1/users' && method === 'GET') {
      await route.fulfill({ json: users });
      return;
    }
    if (path === '/api/v1/users' && method === 'POST') {
      sequence += 1;
      const body = request.postDataJSON() as {
        email: string;
        name: string;
        role: Role;
        token_expires_in_days?: number;
      };
      const days = body.token_expires_in_days ?? 30;
      const created: MockUser = {
        id: `user_created_${sequence}`,
        email: body.email,
        name: body.name,
        created_at: timestamp(0),
        updated_at: timestamp(0),
        token_issued_at: timestamp(0),
        token_expires_at: days === 0 ? null : timestamp(days),
      };
      users.push(created);
      members.push(memberOf(created, body.role));
      await route.fulfill({
        json: {
          user: created,
          membership: {
            org_id: ORG.id,
            user_id: created.id,
            role: body.role,
            created_at: created.created_at,
            updated_at: created.updated_at,
          },
          token: `flu_created_${sequence}`,
        },
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
      const memberIndex = members.findIndex((candidate) => candidate.user_id === remove[1]);
      if (memberIndex >= 0) members.splice(memberIndex, 1);
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

test('admin creates, rotates and deletes a user with a one-time token reveal', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('/users');

  await expect(page.getByRole('heading', { name: 'Users' })).toBeVisible();
  await expect(page.getByTestId('user-row')).toHaveCount(1);
  await expect(page.getByRole('link', { name: 'Users' })).toBeVisible();

  // Create with a short expiry.
  await page.getByRole('button', { name: 'Create user' }).click();
  const createDialog = page.getByRole('dialog');
  await createDialog.getByLabel('Name').fill('Grace Hopper');
  await createDialog.getByLabel('Email').fill('grace@example.com');
  await createDialog.getByLabel('Role').click();
  await page.getByRole('option', { name: 'Member' }).click();
  await createDialog.getByLabel('Token expiry').click();
  await page.getByRole('option', { name: '7 days' }).click();
  await createDialog.getByRole('button', { name: 'Create user' }).click();

  const createToken = createDialog.getByTestId('revealed-token');
  await expect(createToken).toHaveText('flu_created_1');
  await createDialog.getByRole('button', { name: 'Done' }).click();

  const graceRow = page.getByTestId('user-row').filter({ hasText: 'Grace Hopper' });
  await expect(graceRow).toContainText('grace@example.com');
  await expect(graceRow).toContainText('Member');
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

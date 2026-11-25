import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const USER_TOKEN = 'flu_test_user';

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
  org_id: 'org_default',
  role: 'admin',
};

interface Identity {
  id: string;
  provider: string;
  email: string;
  created_at: string;
  last_login_at?: string | null;
}

/**
 * Mock the signed-in shell plus the linked-identity endpoints. The DELETE
 * handler removes the row, and `lastMethod` makes it mimic the server's
 * refusal to unlink the final way to sign in.
 */
async function mockIdentityApi(page: Page, identities: Identity[], lastMethod = false) {
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    if (path === '/api/v1/auth/providers') {
      await route.fulfill({
        json: { providers: [], bootstrap_required: false, bootstrap_email_configured: true },
      });
      return;
    }
    if (path === '/api/v1/auth/session' && method === 'POST') {
      await route.fulfill({ status: 200, json: ME });
      return;
    }
    if (path === '/api/v1/me') {
      await route.fulfill({ json: ME });
      return;
    }
    if (path === '/api/v1/realtime/ticket') {
      await route.fulfill({
        json: { ticket: 'test-ticket', org_id: 'org_default', expires_in: 60 },
      });
      return;
    }
    if (path === '/api/v1/orgs' || path === '/api/v1/machines' || path === '/api/v1/sessions') {
      await route.fulfill({ json: [] });
      return;
    }
    if (path === '/api/v1/me/identities' && method === 'GET') {
      await route.fulfill({ json: identities });
      return;
    }
    if (path.startsWith('/api/v1/me/identities/') && method === 'DELETE') {
      if (lastMethod) {
        await route.fulfill({
          status: 409,
          json: {
            code: 'conflict',
            message:
              'this is the last way to sign in; keep a user token or link another identity first',
          },
        });
        return;
      }
      const id = path.split('/').pop();
      const index = identities.findIndex((identity) => identity.id === id);
      if (index >= 0) identities.splice(index, 1);
      await route.fulfill({ status: 204, body: '' });
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

test('lists linked identities and unlinks one after confirmation', async ({ page }) => {
  const identities: Identity[] = [
    {
      id: 'ident_github',
      provider: 'github',
      email: 'ada@example.com',
      created_at: '2026-01-02T00:00:00Z',
      last_login_at: '2026-01-03T00:00:00Z',
    },
    {
      id: 'ident_google',
      provider: 'google',
      email: 'ada@example.com',
      created_at: '2026-01-04T00:00:00Z',
    },
  ];
  const requests: string[] = [];
  page.on('request', (request) => {
    if (request.method() === 'DELETE') requests.push(request.url());
  });
  await mockIdentityApi(page, identities);

  await page.goto('/sessions');
  await page.getByRole('button', { name: 'Connection settings' }).click();

  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading', { name: 'Sign-in methods' })).toBeVisible();
  const githubRow = dialog.getByRole('listitem').filter({ hasText: 'GitHub' });
  await expect(githubRow).toContainText('ada@example.com');
  await expect(githubRow).toContainText('Last sign-in');
  await expect(dialog.getByRole('listitem').filter({ hasText: 'Google' })).toBeVisible();

  await githubRow.getByRole('button', { name: 'Unlink' }).click();
  const confirm = page.getByRole('dialog', { name: 'Unlink GitHub?' });
  await expect(confirm).toBeVisible();
  await confirm.getByRole('button', { name: 'Unlink' }).click();

  await expect(dialog.getByRole('listitem').filter({ hasText: 'GitHub' })).toHaveCount(0);
  await expect(dialog.getByRole('listitem').filter({ hasText: 'Google' })).toBeVisible();
  expect(requests).toHaveLength(1);
  expect(requests[0]).toContain('/api/v1/me/identities/ident_github');
});

test('surfaces the refusal to unlink the last sign-in method', async ({ page }) => {
  await mockIdentityApi(
    page,
    [
      {
        id: 'ident_github',
        provider: 'github',
        email: 'ada@example.com',
        created_at: '2026-01-02T00:00:00Z',
      },
    ],
    true,
  );

  await page.goto('/sessions');
  await page.getByRole('button', { name: 'Connection settings' }).click();
  const dialog = page.getByRole('dialog');
  const githubRow = dialog.getByRole('listitem').filter({ hasText: 'GitHub' });

  await githubRow.getByRole('button', { name: 'Unlink' }).click();
  const confirm = page.getByRole('dialog', { name: 'Unlink GitHub?' });
  await confirm.getByRole('button', { name: 'Unlink' }).click();

  await expect(confirm.getByText(/last way to sign in/)).toBeVisible();
  // The confirmation stays open; close it before asserting on the settings
  // dialog, which is hidden from the accessibility tree while it is on top.
  await confirm.getByRole('button', { name: 'Cancel' }).click();
  await expect(confirm).toBeHidden();
  await expect(dialog.getByRole('listitem').filter({ hasText: 'GitHub' })).toBeVisible();
});

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

/**
 * A user token is upgraded to an HttpOnly session cookie and removed from
 * localStorage, so the browser keeps no long-lived credential in JavaScript.
 */
async function mockSessionApi(page: Page) {
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;

    if (path === '/api/v1/auth/session' && request.method() === 'POST') {
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
    if (path === '/api/v1/realtime/ticket') {
      await route.fulfill({
        json: { ticket: 'test-ticket', org_id: 'org_default', expires_in: 60 },
      });
      return;
    }
    if (path === '/api/v1/sessions' || path === '/api/v1/machines' || path === '/api/v1/orgs') {
      await route.fulfill({ json: [] });
      return;
    }
    await route.fulfill({
      status: 404,
      json: { code: 'not_found', message: `unmocked request: ${path}` },
    });
  });
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(
    ([key, token]) => window.localStorage.setItem(key, token),
    [TOKEN_STORAGE_KEY, USER_TOKEN],
  );
});

test('exchanges a user token for a session and clears the stored credential', async ({ page }) => {
  await mockSessionApi(page);
  await page.goto('/sessions');

  await expect(page.getByText('AGENT FLEET')).toBeVisible();
  await expect(page.getByTestId('auth-gate')).toHaveCount(0);

  await expect
    .poll(() => page.evaluate((key) => window.localStorage.getItem(key), TOKEN_STORAGE_KEY))
    .toBeNull();
});

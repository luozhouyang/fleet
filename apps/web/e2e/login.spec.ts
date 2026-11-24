import { expect, test, type Page } from '@playwright/test';

async function mockLoginApi(page: Page) {
  await page.route('**/api/v1/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/auth/providers') {
      await route.fulfill({
        json: {
          providers: ['github', 'google'],
          bootstrap_required: true,
          bootstrap_email_configured: true,
        },
      });
      return;
    }
    if (path === '/api/v1/me') {
      await route.fulfill({
        status: 401,
        json: { code: 'unauthorized', message: 'missing or invalid bearer token' },
      });
      return;
    }
    await route.fulfill({
      status: 404,
      json: { code: 'not_found', message: `unmocked request: ${path}` },
    });
  });
}

test('offers the configured providers and explains a callback error', async ({ page }) => {
  await mockLoginApi(page);
  await page.goto('/?auth_error=email_unverified');

  await expect(page.getByTestId('auth-gate')).toBeVisible();
  const github = page.getByTestId('oauth-login-github');
  await expect(github).toBeVisible();
  await expect(github).toHaveAttribute('href', /\/api\/v1\/auth\/login\/github$/);
  await expect(page.getByTestId('oauth-login-google')).toBeVisible();
  await expect(
    page.getByText('The provider did not report a verified email address.'),
  ).toBeVisible();
  await expect(
    page.getByText('The first sign-in must use a configured bootstrap administrator email.'),
  ).toBeVisible();
});

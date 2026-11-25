import { expect, test, type Page } from '@playwright/test';

interface LoginOptions {
  providers: string[];
  bootstrapRequired: boolean;
  bootstrapEmailConfigured: boolean;
}

async function mockLoginApi(
  page: Page,
  { providers, bootstrapRequired, bootstrapEmailConfigured }: LoginOptions,
) {
  await page.route('**/api/v1/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/auth/providers') {
      await route.fulfill({
        json: {
          providers,
          bootstrap_required: bootstrapRequired,
          bootstrap_email_configured: bootstrapEmailConfigured,
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
  await mockLoginApi(page, {
    providers: ['github', 'google'],
    bootstrapRequired: true,
    bootstrapEmailConfigured: true,
  });
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

  // With OAuth configured the CLI token form is not offered.
  await expect(page.getByTestId('auth-gate').getByLabel('API token')).toHaveCount(0);
  await expect(page.getByText('or use an API token')).toHaveCount(0);
});

test('falls back to the token form when no provider is configured', async ({ page }) => {
  await mockLoginApi(page, {
    providers: [],
    bootstrapRequired: false,
    bootstrapEmailConfigured: false,
  });
  await page.goto('/');

  await expect(page.getByTestId('auth-gate')).toBeVisible();
  await expect(page.getByTestId('auth-gate').getByLabel('API token')).toBeVisible();
  await expect(page.getByTestId('oauth-login-github')).toHaveCount(0);
});

test('keeps the token form when the first account needs a CLI token', async ({ page }) => {
  await mockLoginApi(page, {
    providers: ['github'],
    bootstrapRequired: true,
    bootstrapEmailConfigured: false,
  });
  await page.goto('/');

  await expect(page.getByTestId('auth-gate')).toBeVisible();
  await expect(page.getByTestId('oauth-login-github')).toBeVisible();
  await expect(
    page.getByText(
      'No bootstrap administrator email is configured; use a CLI token to create the first account.',
    ),
  ).toBeVisible();
  await expect(page.getByTestId('auth-gate').getByLabel('API token')).toBeVisible();
});

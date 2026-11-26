import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const USER_TOKEN = 'flu_test_user';
const SESSION_ID = 'sess_prompts';
const T0 = '2026-01-01T10:00:00Z';

const ME = {
  kind: 'user',
  user: {
    id: 'user_ada',
    email: 'ada@example.com',
    name: 'Ada Admin',
    created_at: T0,
    updated_at: T0,
    token_issued_at: T0,
    token_expires_at: null,
  },
  org_id: 'org_default',
  role: 'admin',
};

const session = {
  id: SESSION_ID,
  machine_id: 'mach_prompts',
  provider: 'mock',
  agent_type: 'mock',
  status: 'waiting_for_permission',
  cwd: '/tmp/mock',
  title: 'Prompt fixture',
  created_at: T0,
  updated_at: T0,
};

interface Decision {
  id: string;
  approve: boolean;
  optionId?: string;
}

async function mockApi(page: Page, decisions: Decision[], permissions: unknown[]) {
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();

    if (path === '/api/v1/auth/session' && method === 'POST') {
      await route.fulfill({
        status: 200,
        'set-cookie': 'fleet_session=test-session; Path=/; HttpOnly; SameSite=Lax',
        json: ME,
      });
      return;
    }
    if (path === '/api/v1/me') {
      await route.fulfill({ json: ME });
      return;
    }
    if (
      path === '/api/v1/orgs' ||
      path === '/api/v1/machines' ||
      path === '/api/v1/session-folders'
    ) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path === '/api/v1/realtime/ticket') {
      await route.fulfill({
        json: { ticket: 'test-ticket', org_id: 'org_default', expires_in: 60 },
      });
      return;
    }
    if (path === `/api/v1/sessions/${SESSION_ID}/permissions` && method === 'GET') {
      await route.fulfill({ json: permissions });
      return;
    }
    const decisionMatch =
      /^\/api\/v1\/sessions\/[^/]+\/permissions\/([^/]+)\/(approve|reject)$/.exec(path);
    if (decisionMatch && method === 'POST') {
      const body = request.postDataJSON() as { option_id?: string };
      decisions.push({
        id: decisionMatch[1],
        approve: decisionMatch[2] === 'approve',
        optionId: body.option_id,
      });
      await route.fulfill({ json: { ok: true } });
      return;
    }
    if (path === `/api/v1/sessions/${SESSION_ID}/questions`) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path === `/api/v1/sessions/${SESSION_ID}`) {
      await route.fulfill({
        json: {
          session,
          turns: [],
          messages: [],
          tool_calls: [],
          permissions,
          questions: [],
        },
      });
      return;
    }
    if (path === '/api/v1/sessions/facets') {
      await route.fulfill({
        json: {
          all: 1,
          running: 0,
          waiting: 1,
          failed: 0,
          closed: 0,
          archived: 0,
          unread: 0,
          pinned: 0,
        },
      });
      return;
    }
    if (path === '/api/v1/sessions') {
      if (url.searchParams.has('limit') || url.searchParams.has('sort')) {
        await route.fulfill({ json: { items: [session] } });
      } else {
        await route.fulfill({ json: [session] });
      }
      return;
    }
    await route.fulfill({
      status: 404,
      json: { code: 'not_found', message: `unmocked request: ${method} ${path}` },
    });
  });
}

function permission(id: string, toolName: string, createdAt: string) {
  return {
    id,
    session_id: SESSION_ID,
    tool_name: toolName,
    description: `Run ${toolName}`,
    input: { command: 'ls' },
    options: [
      { option_id: `${id}-once`, name: 'Allow once', kind: 'allow_once' },
      { option_id: `${id}-deny`, name: 'Reject', kind: 'reject_once' },
    ],
    status: 'pending',
    created_at: createdAt,
    expires_at: '2026-01-01T10:10:00Z',
  };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(
    ([key, token]) => window.localStorage.setItem(key, token),
    [TOKEN_STORAGE_KEY, USER_TOKEN],
  );
});

test('decides a permission with the keyboard and never steals typed keys', async ({ page }) => {
  const decisions: Decision[] = [];
  await mockApi(page, decisions, [permission('perm_1', 'Bash', T0)]);
  await page.goto(`/sessions/${SESSION_ID}`);

  const stack = page.getByTestId('prompt-stack');
  await expect(stack).toBeVisible();
  await expect(stack).toContainText('Bash');

  // A typed `a` in the composer must not approve anything.
  const composer = page.getByLabel('Message the agent');
  await composer.fill('a');
  await expect(stack).toBeVisible();
  expect(decisions).toHaveLength(0);

  await composer.fill('');
  await page.locator('body').click({ position: { x: 5, y: 5 } });
  await page.keyboard.press('a');
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0]).toEqual({ id: 'perm_1', approve: true, optionId: 'perm_1-once' });
});

test('rejects with the r shortcut', async ({ page }) => {
  const decisions: Decision[] = [];
  await mockApi(page, decisions, [permission('perm_2', 'Bash', T0)]);
  await page.goto(`/sessions/${SESSION_ID}`);
  await expect(page.getByTestId('prompt-stack')).toBeVisible();

  await page.locator('body').click({ position: { x: 5, y: 5 } });
  await page.keyboard.press('r');
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0]).toEqual({ id: 'perm_2', approve: false, optionId: 'perm_2-deny' });
});

test('stacks several permissions, oldest expanded, and bulk-approves reads only', async ({
  page,
}) => {
  const decisions: Decision[] = [];
  const permissions = [
    permission('read_1', 'Read', T0),
    permission('read_2', 'Search', '2026-01-01T10:00:01Z'),
  ];
  await mockApi(page, decisions, permissions);
  await page.goto(`/sessions/${SESSION_ID}`);

  const stack = page.getByTestId('prompt-stack');
  await expect(stack).toContainText('2 permissions waiting');
  await expect(stack).toContainText('Read');

  const bulk = stack.getByRole('button', { name: 'Approve all reads' });
  await expect(bulk).toBeVisible();
  await bulk.click();
  await expect.poll(() => decisions.length).toBe(2);
  expect(decisions.map((decision) => decision.id).sort()).toEqual(['read_1', 'read_2']);
  expect(decisions.every((decision) => decision.approve)).toBe(true);
});

test('withholds bulk approve when a command is waiting', async ({ page }) => {
  const decisions: Decision[] = [];
  await mockApi(page, decisions, [
    permission('read_1', 'Read', T0),
    permission('bash_1', 'Bash', '2026-01-01T10:00:01Z'),
  ]);
  await page.goto(`/sessions/${SESSION_ID}`);

  const stack = page.getByTestId('prompt-stack');
  await expect(stack).toContainText('2 permissions waiting');
  await expect(stack.getByRole('button', { name: 'Approve all reads' })).toHaveCount(0);
});

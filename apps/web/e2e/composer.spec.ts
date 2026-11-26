import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const USER_TOKEN = 'flu_test_user';
const SESSION_ID = 'sess_composer';
const WORKSPACE_ID = 'ws_composer';

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

const session = {
  id: SESSION_ID,
  machine_id: 'mach_composer',
  workspace_id: WORKSPACE_ID,
  provider: 'mock',
  agent_type: 'mock',
  status: 'running',
  cwd: '/tmp/mock',
  title: 'Composer fixture',
  created_at: '2026-01-01T10:00:00Z',
  updated_at: '2026-01-01T10:00:00Z',
};

interface SentMessage {
  content: string;
  attachments?: { name: string; mime_type?: string; data_base64: string }[];
}

async function mockApi(page: Page, sent: SentMessage[]) {
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;

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
    if (
      path === `/api/v1/sessions/${SESSION_ID}/permissions` ||
      path === `/api/v1/sessions/${SESSION_ID}/questions`
    ) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path === `/api/v1/sessions/${SESSION_ID}/messages` && request.method() === 'POST') {
      const body = request.postDataJSON() as SentMessage;
      sent.push(body);
      await route.fulfill({ json: { ok: true } });
      return;
    }
    if (path === `/api/v1/workspaces/${WORKSPACE_ID}/search`) {
      await route.fulfill({
        json: {
          matches: [
            { path: 'src/main.rs', line_number: 1, line: 'fn main() {' },
            { path: 'src/lib.rs', line_number: 3, line: 'pub mod app;' },
          ],
          truncated: false,
        },
      });
      return;
    }
    if (path === `/api/v1/sessions/${SESSION_ID}`) {
      await route.fulfill({
        json: {
          session,
          turns: [],
          messages: [],
          tool_calls: [],
          permissions: [],
          questions: [],
        },
      });
      return;
    }
    if (path === '/api/v1/sessions/facets') {
      await route.fulfill({
        json: {
          all: 1,
          running: 1,
          waiting: 0,
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

test('restores a draft and sends attachments with the message', async ({ page }) => {
  const sent: SentMessage[] = [];
  await mockApi(page, sent);
  await page.addInitScript(() => {
    // Drafts are stored as JSON strings.
    window.localStorage.setItem('fleet.draft.sess_composer', JSON.stringify('unfinished thought'));
  });

  await page.goto(`/sessions/${SESSION_ID}`);
  const composer = page.getByLabel('Message the agent');
  await expect(composer).toHaveValue('unfinished thought');

  // A text file attached through the hidden input travels in the POST body.
  await composer.fill('summarize the diff');
  await page.setInputFiles('input[type="file"]', {
    name: 'notes.md',
    mimeType: 'text/markdown',
    buffer: Buffer.from('# notes\n'),
  });
  await expect(page.getByLabel('Attachments')).toContainText('notes.md');

  await page.getByRole('button', { name: 'Send message' }).click();
  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0].content).toBe('summarize the diff');
  expect(sent[0].attachments).toHaveLength(1);
  expect(sent[0].attachments?.[0].name).toBe('notes.md');
  expect(sent[0].attachments?.[0].data_base64).toBeTruthy();

  // The draft is cleared once the message is accepted.
  await expect(composer).toHaveValue('');
  await expect(page.getByLabel('Attachments')).toHaveCount(0);
});

test('recalls prompt history with the arrow keys', async ({ page }) => {
  const sent: SentMessage[] = [];
  await mockApi(page, sent);
  await page.goto(`/sessions/${SESSION_ID}`);

  const composer = page.getByLabel('Message the agent');
  await composer.fill('first prompt');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect.poll(() => sent.length).toBe(1);

  await composer.fill('second prompt');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect.poll(() => sent.length).toBe(2);
  await expect(composer).toHaveValue('');

  // The caret sits at the start of an empty composer, so ArrowUp recalls.
  await composer.press('ArrowUp');
  await expect(composer).toHaveValue('second prompt');
  await composer.press('ArrowUp');
  await expect(composer).toHaveValue('first prompt');
  await composer.press('ArrowDown');
  await expect(composer).toHaveValue('second prompt');
});

test('offers slash commands and workspace mentions from the pickers', async ({ page }) => {
  const sent: SentMessage[] = [];
  await mockApi(page, sent);
  await page.goto(`/sessions/${SESSION_ID}`);

  const composer = page.getByLabel('Message the agent');
  await composer.fill('/int');
  const commandPicker = page.getByRole('listbox', { name: 'Composer commands' });
  await expect(commandPicker).toBeVisible();
  await expect(commandPicker.getByRole('option', { name: /interrupt/ })).toBeVisible();
  await composer.press('Enter');
  await expect(commandPicker).toBeHidden();

  await composer.fill('look at @main');
  const mentionPicker = page.getByRole('listbox', { name: 'Workspace files' });
  await expect(mentionPicker).toBeVisible();
  await mentionPicker.getByRole('option', { name: 'src/main.rs' }).click();
  await expect(composer).toHaveValue('look at @src/main.rs ');
});

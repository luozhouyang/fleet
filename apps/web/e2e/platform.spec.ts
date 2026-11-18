import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { appendFileSync } from 'node:fs';
import { join } from 'node:path';

const TOKEN = process.env.E2E_TOKEN ?? 'dev-token';
const REPO = process.env.E2E_REPO;
const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';

const authHeaders = { Authorization: `Bearer ${TOKEN}` };

interface WorkspaceSummary {
  id: string;
  name: string;
  root_path: string;
}

interface MachineSummary {
  id: string;
  name: string;
  status: string;
  providers?: { id: string; display_name: string }[];
  workspaces?: WorkspaceSummary[];
}

interface BranchSummary {
  name: string;
  is_current: boolean;
  is_remote: boolean;
}

async function onlineMachine(request: APIRequestContext): Promise<MachineSummary> {
  const response = await request.get('/api/v1/machines', { headers: authHeaders });
  expect(response.ok(), `GET /machines returned ${response.status()}`).toBeTruthy();
  const machines = (await response.json()) as MachineSummary[];
  const machine = machines.find((candidate) => candidate.status === 'online');
  expect(machine, 'the daemon should be online before the UI tests run').toBeTruthy();
  return machine as MachineSummary;
}

async function machineDetail(
  request: APIRequestContext,
  machineId: string,
): Promise<MachineSummary> {
  const response = await request.get(`/api/v1/machines/${machineId}`, {
    headers: authHeaders,
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as MachineSummary;
}

async function firstWorkspace(request: APIRequestContext): Promise<WorkspaceSummary> {
  const machine = await onlineMachine(request);
  const detail = await machineDetail(request, machine.id);
  const workspace = detail.workspaces?.[0];
  expect(workspace, 'a workspace must be registered before the UI tests run').toBeTruthy();
  return workspace as WorkspaceSummary;
}

async function gitBranches(
  request: APIRequestContext,
  workspaceId: string,
): Promise<BranchSummary[]> {
  const response = await request.get(`/api/v1/workspaces/${workspaceId}/git/branches`, {
    headers: authHeaders,
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as BranchSummary[];
}

interface MockSession {
  id: string;
  machine_id: string;
  workspace_id?: string;
  provider: string;
  agent_type: string;
  status: string;
  cwd: string;
  created_at: string;
  updated_at: string;
  metadata?: Record<string, unknown>;
}

interface MockPermissionOption {
  option_id: string;
  name: string;
  kind: string;
}

interface MockPermission {
  id: string;
  session_id: string;
  tool_name: string;
  description: string;
  input: unknown;
  options?: MockPermissionOption[];
  status: string;
  created_at: string;
}

interface MockQuestionRequest {
  id: string;
  session_id: string;
  questions: {
    id: string;
    header?: string;
    question: string;
    options?: { label: string; description?: string }[];
    multi_select?: boolean;
    allow_custom?: boolean;
    is_secret?: boolean;
  }[];
  status: string;
  created_at: string;
}

function mockSession(overrides: Partial<MockSession>): MockSession {
  const now = new Date().toISOString();
  return {
    id: 'sess_ui_mock',
    machine_id: 'mach_ui_mock',
    workspace_id: 'ws_ui_mock',
    provider: 'mock',
    agent_type: 'mock',
    status: 'running',
    cwd: '/tmp/mock',
    created_at: now,
    updated_at: now,
    ...overrides,
  };
}

async function installSessionMocks(
  page: Page,
  session: MockSession,
  options: {
    deleteError?: string;
    permissions?: MockPermission[];
    questions?: MockQuestionRequest[];
  } = {},
): Promise<void> {
  const now = new Date().toISOString();
  const permissions = options.permissions ?? [];
  const questions = options.questions ?? [];
  const machine = {
    id: session.machine_id,
    name: 'UI Mock Machine',
    hostname: 'ui-mock-host',
    os: 'linux',
    arch: 'x86_64',
    daemon_version: '0.1.0',
    protocol_version: 1,
    status: 'offline',
    created_at: now,
    updated_at: now,
    workspace_count: 1,
    workspaces: session.workspace_id
      ? [
          {
            id: session.workspace_id,
            name: 'Mock Workspace',
            root_path: '/tmp/mock',
            is_git_repository: true,
          },
        ]
      : [],
    providers: [
      {
        id: 'mock',
        kind: 'mock',
        display_name: 'Mock Agent',
        version: '0.1.0',
        available: true,
        capabilities: {
          streaming: true,
          interrupt: true,
          resume: true,
          permission_approval: true,
          structured_events: true,
          structured_diff: true,
          session_persistence: true,
          tool_calls: true,
          images: false,
          user_questions: true,
        },
      },
    ],
  };

  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;

    if (request.method() === 'DELETE' && path === `/api/v1/sessions/${session.id}`) {
      await route.fulfill({
        status: 503,
        json: {
          code: 'machine_offline',
          message: options.deleteError ?? 'the session machine is offline',
        },
      });
      return;
    }
    if (path.endsWith('/permissions')) {
      await route.fulfill({ json: permissions });
      return;
    }
    if (path.endsWith('/questions')) {
      await route.fulfill({ json: questions });
      return;
    }
    if (path === `/api/v1/sessions/${session.id}`) {
      await route.fulfill({
        json: {
          session,
          turns: [],
          messages: [],
          tool_calls: [],
          permissions,
          questions,
        },
      });
      return;
    }
    if (path === '/api/v1/sessions') {
      await route.fulfill({ json: [session] });
      return;
    }
    if (path === '/api/v1/machines') {
      await route.fulfill({ json: [machine] });
      return;
    }
    await route.fulfill({
      status: 404,
      json: { code: 'not_found', message: `unmocked request: ${path}` },
    });
  });
}

async function createMockSession(
  page: Page,
  request: APIRequestContext,
): Promise<{ id: string }> {
  const workspace = await firstWorkspace(request);
  const machine = await onlineMachine(request);

  await page.goto('/sessions');
  await page.getByRole('button', { name: 'New session', exact: true }).first().click();

  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();

  await dialog.getByLabel('Machine').click();
  await page.getByRole('option').filter({ hasText: machine.name }).first().click();

  const agentTrigger = dialog.getByLabel('Agent');
  await agentTrigger.click();
  await page.getByRole('option').filter({ hasText: 'Mock Agent' }).first().click();
  await expect(agentTrigger).toContainText('Mock Agent');

  await dialog.getByLabel('Workspace').click();
  await page.getByRole('option').filter({ hasText: workspace.name }).first().click();

  await dialog.getByRole('button', { name: 'Start session' }).click();
  await expect(page).toHaveURL(/\/sessions\/[^/]+$/, { timeout: 20_000 });

  const id = new URL(page.url()).pathname.split('/').pop();
  expect(id, 'the created session id').toBeTruthy();
  return { id: id as string };
}

async function sendMessage(page: Page, sessionId: string, content: string): Promise<void> {
  await expect(page).toHaveURL(new RegExp(`/sessions/${sessionId}$`));
  const composer = page.getByLabel('Message the agent');
  await expect(composer).toBeVisible();
  await composer.fill(content);
  await page.getByRole('button', { name: 'Send message' }).click();
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(([key, token]) => window.localStorage.setItem(key, token), [
    TOKEN_STORAGE_KEY,
    TOKEN,
  ] as const);
});

test('machines ledger opens a machine and lists its providers', async ({ page, request }) => {
  const machine = await onlineMachine(request);

  await page.goto('/machines');
  await expect(page.getByRole('heading', { name: 'Machines' })).toBeVisible();

  const row = page.locator(`[data-testid="machine-row"][data-id="${machine.id}"]`);
  await expect(row).toBeVisible();
  await row.click();

  await expect(page).toHaveURL(new RegExp(`/machines/${machine.id}$`));
  await expect(page.getByRole('heading', { name: machine.name })).toBeVisible();
  await expect(page.getByText('Mock Agent').first()).toBeVisible();
  await expect(page.getByText('Available', { exact: true }).first()).toBeVisible();
});

test('workspace explorer opens files and renders git diffs', async ({ page, request }) => {
  const workspace = await firstWorkspace(request);

  await page.goto(`/workspaces/${workspace.id}`);
  await expect(page.getByRole('heading', { name: workspace.name })).toBeVisible();

  const treeEntry = (path: string) =>
    page.locator(`[data-testid="tree-entry"][data-path="${path}"]`);

  await expect(treeEntry('a.txt')).toBeVisible();
  await treeEntry('a.txt').click();
  await expect(page.locator('.monaco-editor').first()).toBeVisible();
  await expect(page.locator('.view-lines').first()).toContainText('hello');

  await treeEntry('src').click();
  await treeEntry('src/main.rs').click();
  await expect(page.locator('.view-lines').first()).toContainText('fn main');

  await expect(page.locator('[data-testid="git-file"][data-path="a.txt"]')).toBeVisible();
  await page.locator('[data-testid="git-file"][data-path="a.txt"]').first().click();
  await expect(page.locator('.monaco-diff-editor')).toBeVisible();
});

test('creates a mock session, streams a reply, approves a permission and interrupts', async ({
  page,
  request,
}) => {
  const workspace = await firstWorkspace(request);
  const machine = await onlineMachine(request);

  await page.goto('/sessions');
  await page.getByRole('button', { name: 'New session', exact: true }).first().click();

  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();

  await dialog.getByLabel('Machine').click();
  await page.getByRole('option').filter({ hasText: machine.name }).first().click();

  const agentTrigger = dialog.getByLabel('Agent');
  await agentTrigger.click();
  await page.getByRole('option').filter({ hasText: 'Mock Agent' }).first().click();
  await expect(agentTrigger).toContainText('Mock Agent');

  await dialog.getByLabel('Workspace').click();
  await page.getByRole('option').filter({ hasText: workspace.name }).first().click();

  await dialog.getByRole('button', { name: 'Start session' }).click();

  await expect(page).toHaveURL(/\/sessions\/[^/]+$/, { timeout: 20_000 });

  // The context panel exposes files, diffs and session details for the session.
  const context = page.locator('aside[aria-label="Session context"]');
  await expect(context).toBeVisible();
  const contextTreeEntry = context.locator('[data-testid="session-tree-entry"][data-path="a.txt"]');
  await expect(contextTreeEntry).toBeVisible({ timeout: 20_000 });
  await contextTreeEntry.click();
  await expect(context).toContainText('changed');

  await context.getByRole('tab', { name: 'Diff' }).click();
  await expect(context.locator('[data-testid="session-diff-file"][data-path="a.txt"]')).toBeVisible(
    { timeout: 20_000 },
  );

  await context.getByRole('tab', { name: 'System' }).click();
  await expect(context).toContainText('mock');

  await context.getByRole('tab', { name: 'File' }).click();

  const composer = page.getByLabel('Message the agent');
  await expect(composer).toBeEnabled();

  // Plain message: the mock streams `Echo: <text>` word by word.
  await composer.fill('hello from playwright');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect(page.getByText('Echo: hello from playwright')).toBeVisible({
    timeout: 20_000,
  });

  // Tool scenario: a permission card appears, the approval dialog completes it.
  await composer.fill('/tool');
  await page.getByRole('button', { name: 'Send message' }).click();

  // The optimistic user message is replaced by the daemon-confirmed message;
  // it must appear exactly once.
  await expect(page.getByText('/tool', { exact: true })).toHaveCount(1);

  const permissionCard = page.getByTestId('permission-card');
  await expect(permissionCard).toBeVisible({ timeout: 20_000 });
  await expect(permissionCard.getByText('Permission required')).toBeVisible();
  await expect(permissionCard.getByText('Bash', { exact: true })).toBeVisible();

  await permissionCard.getByRole('button', { name: 'Approve', exact: true }).click();
  const approvalDialog = page.getByRole('dialog', { name: 'Approve this action?' });
  await expect(approvalDialog).toBeVisible();
  // The mock provider advertises explicit permission options.
  await expect(approvalDialog.getByRole('button', { name: 'Allow once' })).toBeVisible();
  await expect(approvalDialog.getByRole('button', { name: 'Allow always' })).toBeVisible();
  await approvalDialog.getByRole('button', { name: 'Allow once' }).click();
  await expect(approvalDialog).toBeHidden();

  await expect(permissionCard.getByText('Approved', { exact: true }).first()).toBeVisible({
    timeout: 20_000,
  });
  await expect(page.getByText('Tool finished.')).toBeVisible({ timeout: 20_000 });

  // The daemon is the transcript source; the admin proxies it. Reloading must
  // restore both user messages and the structured tool call from history.
  const sessionId = new URL(page.url()).pathname.split('/').pop();
  const detailResponse = await request.get(`/api/v1/sessions/${sessionId}`, {
    headers: authHeaders,
  });
  expect(detailResponse.ok()).toBeTruthy();
  const detail = (await detailResponse.json()) as {
    messages: { role: string; content: string }[];
    tool_calls: { name: string; status: string }[];
  };
  expect(detail.messages).toEqual(
    expect.arrayContaining([expect.objectContaining({ role: 'user', content: '/tool' })]),
  );
  expect(detail.tool_calls).toEqual(
    expect.arrayContaining([expect.objectContaining({ name: 'Bash', status: 'completed' })]),
  );

  await page.reload();
  await expect(page.getByTestId('tool-card').filter({ hasText: 'Bash' })).toBeVisible({
    timeout: 20_000,
  });
  await expect(page.getByText('Tool finished.')).toBeVisible({ timeout: 20_000 });

  // Long-running turn: interrupt it from the session header.
  await composer.fill('/never');
  await page.getByRole('button', { name: 'Send message' }).click();

  const interrupt = page.getByRole('button', { name: 'Interrupt' });
  await expect(interrupt).toBeEnabled({ timeout: 20_000 });
  await interrupt.click();
  await expect(page.getByText('Interrupted', { exact: true }).first()).toBeVisible({
    timeout: 20_000,
  });

  // Closing terminates the session and returns to the session list.
  await page.getByRole('button', { name: 'Close session', exact: true }).click();
  const closeDialog = page.getByRole('dialog', { name: 'Close session?' });
  await expect(closeDialog).toBeVisible();
  await closeDialog.getByRole('button', { name: 'Close session' }).click();
  await expect(page).toHaveURL(/\/sessions$/, { timeout: 20_000 });
  await expect(composer).toBeHidden();
});

test('stages, commits and checks out branches from the Git panel', async ({ page, request }) => {
  test.skip(!REPO, 'E2E_REPO is required for the Git panel test');
  const workspace = await firstWorkspace(request);
  const workspaceId = workspace.id;

  // Make the test retry-safe: return to the default branch before starting.
  const branches = await gitBranches(request, workspaceId);
  const localBranches = branches.filter((branch) => !branch.is_remote);
  const baseBranch =
    localBranches.find((branch) => branch.name === 'main' || branch.name === 'master')?.name ??
    localBranches.find((branch) => branch.is_current)?.name;
  expect(baseBranch, 'the fixture repository must have a branch').toBeTruthy();

  const checkout = await request.post(`/api/v1/workspaces/${workspaceId}/git/checkout`, {
    headers: authHeaders,
    data: { branch: baseBranch },
  });
  expect(checkout.ok()).toBeTruthy();

  appendFileSync(join(REPO as string, 'a.txt'), 'playwright git change\n');
  const commitMessage = `ui: commit from playwright ${Date.now()}`;
  const branchName = `playwright/ui-${Date.now()}`;

  await page.goto(`/workspaces/${workspaceId}`);
  await expect(page.getByTestId('git-branch')).toHaveText(baseBranch as string);

  const stage = page.locator('[data-testid="git-stage"][data-path="a.txt"]');
  await expect(stage).toBeVisible({ timeout: 20_000 });

  // Stage, unstage, then stage again.
  await stage.click();
  const unstage = page.locator('[data-testid="git-unstage"][data-path="a.txt"]');
  await expect(unstage).toBeVisible();
  await unstage.click();
  await expect(page.locator('[data-testid="git-stage"][data-path="a.txt"]')).toBeVisible();
  await page.locator('[data-testid="git-stage"][data-path="a.txt"]').click();
  await expect(unstage).toBeVisible();

  // Commit the staged file.
  await page.getByTestId('git-commit-message').fill(commitMessage);
  await page.getByTestId('git-commit-submit').click();
  await expect(page.getByText(/Committed [0-9a-f]{7}/)).toBeVisible({ timeout: 20_000 });
  await expect(page.locator('[data-testid="git-unstage"][data-path="a.txt"]')).toHaveCount(0);

  const logResponse = await request.get(`/api/v1/workspaces/${workspaceId}/git/log?limit=5`, {
    headers: authHeaders,
  });
  expect(logResponse.ok()).toBeTruthy();
  const log = (await logResponse.json()) as { message: string }[];
  expect(log.some((entry) => entry.message.includes(commitMessage))).toBeTruthy();

  const auditResponse = await request.get('/api/v1/audit?limit=50', {
    headers: authHeaders,
  });
  expect(auditResponse.ok()).toBeTruthy();
  const audit = (await auditResponse.json()) as {
    entries: { action: string; details: { outcome?: string } }[];
  };
  expect(
    audit.entries.some((entry) => entry.action === 'git.commit' && entry.details.outcome === 'ok'),
  ).toBeTruthy();

  // Create a branch from the panel, then switch back.
  await page.getByTestId('git-new-branch').click();
  await page.getByTestId('git-new-branch-input').fill(branchName);
  await page.getByTestId('git-new-branch-submit').click();
  await expect(page.getByTestId('git-branch')).toHaveText(branchName, { timeout: 20_000 });

  await page.getByLabel('Switch branch').click();
  await page
    .getByRole('option')
    .filter({ hasText: baseBranch as string })
    .first()
    .click();

  const switchDialog = page.getByRole('dialog', { name: 'Switch branch?' });
  await expect(switchDialog).toBeVisible();
  await switchDialog.getByRole('button', { name: 'Switch branch' }).click();
  await expect(page.getByTestId('git-branch')).toHaveText(baseBranch as string, {
    timeout: 20_000,
  });
});

test('disables the composer when session history is unavailable', async ({ page }) => {
  // Avoid mounting the context panel, which would request workspace data.
  await page.setViewportSize({ width: 1024, height: 900 });
  const session = mockSession({
    id: 'sess_history_unavailable',
    metadata: { history_unavailable: true },
  });
  await installSessionMocks(page, session);

  await page.goto(`/sessions/${session.id}`);

  const composer = page.getByLabel('Message the agent');
  await expect(composer).toBeVisible();
  await expect(composer).toBeDisabled();
  await expect(
    page.getByText(
      'The machine is unreachable. The transcript lives on the daemon and cannot be loaded right now.',
    ),
  ).toBeVisible();
});

test('offers delete for a completed session and surfaces API failures', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 900 });
  const session = mockSession({ id: 'sess_completed', status: 'completed' });
  await installSessionMocks(page, session);

  await page.goto(`/sessions/${session.id}`);

  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete session permanently?' });
  await expect(dialog).toBeVisible();

  await dialog.getByRole('button', { name: 'Delete session' }).click();
  await expect(dialog.getByText('the session machine is offline')).toBeVisible();
});

test('mock question round trip', async ({ page, request }) => {
  const session = await createMockSession(page, request);
  await sendMessage(page, session.id, '/question');

  const card = page.getByTestId('question-composer');
  await expect(card).toBeVisible({ timeout: 20_000 });
  await expect(card.getByText('1 / 2')).toBeVisible();
  await expect(card.getByRole('radiogroup')).toBeVisible();
  await expect(card.getByRole('button', { name: 'Skip' })).toBeVisible();

  // Single-select answers advance to the next question.
  await card.getByRole('radio', { name: 'staging' }).click();
  await expect(card.getByText('2 / 2')).toBeVisible();
  await card.getByPlaceholder('Type your answer…').fill('deploy after lunch');
  await card.getByRole('button', { name: 'Submit' }).click();

  await expect(card).toBeHidden({ timeout: 20_000 });

  const timeline = page.getByTestId('question-timeline');
  await expect(timeline).toContainText('staging');
  await expect(timeline).toContainText('deploy after lunch');
  await expect(page.getByText(/Question answered:/)).toBeVisible({ timeout: 20_000 });
});

test('mocked question takeover answers and renders the timeline', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 900 });
  const session = mockSession({ id: 'sess_question' });
  const now = new Date().toISOString();
  const question: MockQuestionRequest = {
    id: 'question_mocked',
    session_id: session.id,
    questions: [
      {
        id: 'target',
        header: 'Deploy',
        question: 'Which environment?',
        options: [
          { label: 'staging', description: 'Safe' },
          { label: 'production', description: 'Live traffic' },
        ],
        multi_select: false,
        allow_custom: true,
      },
      {
        id: 'notes',
        question: 'Any deployment notes?',
        options: [],
        multi_select: false,
        allow_custom: true,
      },
    ],
    status: 'pending',
    created_at: now,
  };
  await installSessionMocks(page, session, { questions: [question] });

  let decision: Record<string, unknown> | null = null;
  await page.route('**/api/v1/sessions/*/questions/*/answer', async (route) => {
    decision = route.request().postDataJSON() as Record<string, unknown>;
    await route.fulfill({ json: { status: 'answered' } });
  });

  await page.goto(`/sessions/${session.id}`);

  const card = page.getByTestId('question-composer');
  await expect(card).toBeVisible();
  await expect(card.getByText('1 / 2')).toBeVisible();
  await expect(card.getByRole('radiogroup')).toBeVisible();
  await expect(card.getByRole('button', { name: 'Skip' })).toBeVisible();

  await card.getByRole('radio', { name: 'staging' }).click();
  await expect(card.getByText('2 / 2')).toBeVisible();
  await card.getByPlaceholder('Type your answer…').fill('deploy after lunch');
  await card.getByRole('button', { name: 'Submit' }).click();

  await expect(card).toBeHidden();

  const timeline = page.getByTestId('question-timeline');
  await expect(timeline).toContainText('staging');
  await expect(timeline).toContainText('deploy after lunch');
  await expect.poll(() => decision).toEqual(
    expect.objectContaining({
      answers: expect.arrayContaining([
        expect.objectContaining({ id: 'target', selected: ['staging'] }),
        expect.objectContaining({ id: 'notes', custom: 'deploy after lunch' }),
      ]),
    }),
  );
});

test('permission dialog renders provider options and passes option_id', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 900 });
  const session = mockSession({ id: 'sess_permission_options' });
  const now = new Date().toISOString();
  const permission: MockPermission = {
    id: 'perm_options',
    session_id: session.id,
    tool_name: 'Bash',
    description: 'Allow Bash to run `echo hello`?',
    input: { command: 'echo hello' },
    options: [
      { option_id: 'allow_once', name: 'Allow once', kind: 'allow_once' },
      { option_id: 'allow_always', name: 'Always allow Bash', kind: 'allow_always' },
      { option_id: 'reject_once', name: 'Reject', kind: 'reject_once' },
    ],
    status: 'pending',
    created_at: now,
  };
  await installSessionMocks(page, session, { permissions: [permission] });

  let decision: Record<string, unknown> | null = null;
  await page.route(
    '**/api/v1/sessions/*/permissions/*/approve',
    async (route) => {
      decision = route.request().postDataJSON() as Record<string, unknown>;
      await route.fulfill({ json: { status: 'approved' } });
    },
  );

  await page.goto(`/sessions/${session.id}`);

  const card = page.getByTestId('permission-card');
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: 'Approve', exact: true }).click();

  const dialog = page.getByRole('dialog', { name: 'Approve this action?' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Allow once' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Allow always' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Reject', exact: true })).toBeVisible();

  await dialog.getByRole('button', { name: 'Allow always' }).click();

  await expect.poll(() => decision).toEqual(
    expect.objectContaining({ option_id: 'allow_always' }),
  );
});

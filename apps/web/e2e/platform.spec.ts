import { expect, test, type APIRequestContext } from '@playwright/test';
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

test.beforeEach(async ({ page }) => {
  await page.addInitScript(([key, token]) => window.localStorage.setItem(key, token), [
    TOKEN_STORAGE_KEY,
    TOKEN,
  ] as const);
});

test('machines dashboard shows the connected daemon and its providers', async ({
  page,
  request,
}) => {
  const machine = await onlineMachine(request);

  await page.goto('/machines');

  await expect(page.getByRole('heading', { name: machine.name })).toBeVisible();
  await expect(page.getByText('Online', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('Mock Agent').first()).toBeVisible();
});

test('workspace explorer opens files and renders git diffs', async ({ page, request }) => {
  const machine = await onlineMachine(request);
  const detail = await machineDetail(request, machine.id);
  const workspace = detail.workspaces?.[0];
  expect(workspace, 'a workspace must be registered before the UI tests run').toBeTruthy();

  await page.goto(`/workspaces/${workspace!.id}`);

  await expect(page.getByText('Files', { exact: true })).toBeVisible();
  await expect(page.getByTestId('tree-entry').filter({ hasText: 'a.txt' })).toBeVisible();

  await page.locator('[data-testid="tree-entry"][data-path="src"]').click();
  await page.locator('[data-testid="tree-entry"][data-path="src/main.rs"]').click();

  await expect(page.locator('.monaco-editor').first()).toBeVisible();
  await expect(page.locator('.view-lines').first()).toContainText('fn main');

  await expect(page.getByText('Git changes', { exact: true })).toBeVisible();
  await page.locator('[data-testid="git-file"][data-path="a.txt"]').first().click();
  await expect(page.locator('.monaco-diff-editor')).toBeVisible();
});

test('an open file warns when it changes on disk', async ({ page, request }) => {
  test.skip(!REPO, 'E2E_REPO must point at the live workspace checkout');
  const machine = await onlineMachine(request);
  const detail = await machineDetail(request, machine.id);
  const workspace = detail.workspaces?.[0];
  expect(workspace).toBeTruthy();

  await page.goto(`/workspaces/${workspace!.id}`);
  await page.locator('[data-testid="tree-entry"][data-path="a.txt"]').click();
  await expect(page.locator('.monaco-editor').first()).toBeVisible();

  appendFileSync(join(REPO as string, 'a.txt'), 'edited by playwright\n');

  await expect(page.getByText('File changed on disk', { exact: true })).toBeVisible({
    timeout: 20_000,
  });
});

test('creates a mock session, streams a reply, approves a permission and interrupts', async ({
  page,
  request,
}) => {
  const machine = await onlineMachine(request);
  const detail = await machineDetail(request, machine.id);
  const workspace = detail.workspaces?.[0];
  expect(workspace).toBeTruthy();

  await page.goto(`/machines/${machine.id}`);
  await page.getByRole('button', { name: 'New session', exact: true }).click();

  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.locator('option', { hasText: 'Mock Agent' })).toHaveCount(1);

  await dialog.getByLabel('Provider').selectOption('mock');
  await dialog.getByLabel('Workspace').selectOption(workspace!.id);
  await dialog.getByRole('button', { name: 'Start session' }).click();

  await expect(page).toHaveURL(/\/sessions\/[^/]+$/, { timeout: 20_000 });

  const composer = page.getByLabel('Message the coding agent');
  await expect(composer).toBeEnabled();

  // Plain message: the mock streams `Echo: <text>` word by word.
  await composer.fill('hello from playwright');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect(page.getByText('Echo: hello from playwright')).toBeVisible({
    timeout: 20_000,
  });

  // Tool scenario: a permission card appears and the approval completes the tool.
  await composer.fill('/tool');
  await page.getByRole('button', { name: 'Send message' }).click();

  const permissionCard = page.getByTestId('permission-card');
  await expect(permissionCard).toBeVisible({ timeout: 20_000 });
  await expect(permissionCard.getByText('Permission required')).toBeVisible();
  await expect(permissionCard.getByText('Bash', { exact: true })).toBeVisible();
  await expect(permissionCard.getByText('Pending', { exact: true })).toBeVisible();

  await permissionCard.getByRole('button', { name: 'Approve' }).click();
  await expect(permissionCard.getByText('Approved', { exact: true }).first()).toBeVisible({
    timeout: 20_000,
  });
  await expect(page.getByText('Tool finished.')).toBeVisible({ timeout: 20_000 });

  // Long-running turn: interrupt it from the session header.
  await composer.fill('/never');
  await page.getByRole('button', { name: 'Send message' }).click();

  const interrupt = page.getByRole('button', { name: 'Interrupt' });
  await expect(interrupt).toBeEnabled({ timeout: 20_000 });
  await interrupt.click();
  await expect(page.getByText('Interrupted', { exact: true })).toBeVisible({
    timeout: 20_000,
  });

  // Closing terminates the session and disables the composer. The header
  // button opens a confirmation dialog first.
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  const closeDialog = page.getByRole('dialog', { name: 'Close session?' });
  await expect(closeDialog).toBeVisible();
  await closeDialog.getByRole('button', { name: 'Close session' }).click();
  // Closing deletes the session row, so the page navigates back to the list
  // instead of rendering a terminal status for long.
  await expect(page).toHaveURL(/\/sessions$/, { timeout: 20_000 });
  await expect(composer).toBeHidden();
});

test('stages, commits and checks out branches from the Git panel', async ({ page, request }) => {
  test.skip(!REPO, 'E2E_REPO is required for the Git panel test');
  const machine = await onlineMachine(request);
  const detail = await machineDetail(request, machine.id);
  const workspace = detail.workspaces?.[0];
  expect(workspace).toBeTruthy();
  const workspaceId = workspace!.id;

  const statusResponse = await request.get(`/api/v1/workspaces/${workspaceId}/git/status`, {
    headers: authHeaders,
  });
  expect(statusResponse.ok()).toBeTruthy();
  const initialStatus = (await statusResponse.json()) as { branch?: string };
  const baseBranch = initialStatus.branch;
  expect(baseBranch, 'the fixture repository must have a branch').toBeTruthy();

  // Make the test retry-safe: return to the base branch and ensure a.txt has
  // an unstaged change even if a previous attempt committed it.
  if (initialStatus.branch !== baseBranch) {
    const checkout = await request.post(`/api/v1/workspaces/${workspaceId}/git/checkout`, {
      headers: authHeaders,
      data: { branch: baseBranch },
    });
    expect(checkout.ok()).toBeTruthy();
  }
  appendFileSync(join(REPO as string, 'a.txt'), 'playwright git change\n');
  const commitMessage = `ui: commit from playwright ${Date.now()}`;
  const branchName = `playwright/ui-${Date.now()}`;

  await page.goto(`/workspaces/${workspaceId}`);
  await expect(page.getByText('Git changes', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Refresh Git status' }).click();
  await expect(page.locator('[data-testid="git-stage"][data-path="a.txt"]')).toBeVisible({
    timeout: 20_000,
  });

  // Stage, unstage, then stage again.
  await page.locator('[data-testid="git-stage"][data-path="a.txt"]').click();
  const unstage = page.locator('[data-testid="git-unstage"][data-path="a.txt"]');
  await expect(unstage).toBeVisible();
  await unstage.click();
  await expect(page.locator('[data-testid="git-stage"][data-path="a.txt"]')).toBeVisible();
  await page.locator('[data-testid="git-stage"][data-path="a.txt"]').click();
  await expect(unstage).toBeVisible();

  // Commit the staged file.
  await page.getByTestId('git-commit-open').click();
  const commitDialog = page.getByRole('dialog', { name: 'Commit staged changes' });
  await expect(commitDialog).toBeVisible();
  await page.getByTestId('git-commit-message').fill(commitMessage);
  await page.getByTestId('git-commit-submit').click();
  await expect(commitDialog).toBeHidden();
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
  await page.getByTestId('git-checkout-open').click();
  const checkoutDialog = page.getByRole('dialog', { name: 'Switch branch' });
  await expect(checkoutDialog).toBeVisible();
  await page.getByTestId('git-checkout-new').fill(branchName);
  await page.getByTestId('git-checkout-submit').click();
  await expect(checkoutDialog).toBeHidden();
  await expect(page.getByTestId('git-branch')).toHaveText(branchName);

  await page.getByTestId('git-checkout-open').click();
  await expect(page.getByTestId('git-checkout-branch')).toBeVisible();
  await page.getByTestId('git-checkout-branch').selectOption(baseBranch as string);
  await page.getByTestId('git-checkout-submit').click();
  await expect(page.getByTestId('git-branch')).toHaveText(baseBranch as string);
});

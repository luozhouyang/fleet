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
  await page.getByRole('button', { name: 'New session' }).click();

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
  await expect(page.getByText('Terminated', { exact: true })).toBeVisible({
    timeout: 20_000,
  });
  await expect(composer).toBeHidden();
});

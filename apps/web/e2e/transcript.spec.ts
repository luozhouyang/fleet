import { expect, test, type Page } from '@playwright/test';

const TOKEN_STORAGE_KEY = 'agent-fleet.api-token';
const USER_TOKEN = 'flu_test_user';
const SESSION_ID = 'sess_transcript';

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

interface FakeSocket {
  readyState: number;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: string }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  onclose: ((event: unknown) => void) | null;
  close: (code?: number, reason?: string) => void;
}

declare global {
  interface Window {
    __fleetSockets: FakeSocket[];
  }
}

let eventSequence = 0;

/**
 * Replace the browser WebSocket with a stub the test drives directly. The
 * transcript is fed through the same `agent.event` platform frames the admin
 * gateway sends, so the reducer and row components run exactly as in
 * production without a daemon.
 */
async function installFakeWebSocket(page: Page) {
  await page.addInitScript(() => {
    class TestWebSocket {
      static readonly CONNECTING = 0;
      static readonly OPEN = 1;
      static readonly CLOSING = 2;
      static readonly CLOSED = 3;

      readyState = TestWebSocket.CONNECTING;
      onopen: ((event: unknown) => void) | null = null;
      onmessage: ((event: { data: string }) => void) | null = null;
      onerror: ((event: unknown) => void) | null = null;
      onclose: ((event: unknown) => void) | null = null;

      constructor() {
        window.__fleetSockets.push(this);
        queueMicrotask(() => {
          this.readyState = TestWebSocket.OPEN;
          this.onopen?.({});
        });
      }

      send() {}

      close() {
        this.readyState = TestWebSocket.CLOSED;
        const handler = this.onclose;
        this.onclose = null;
        queueMicrotask(() => handler?.({}));
      }
    }

    window.__fleetSockets = [];
    Object.defineProperty(window, 'WebSocket', {
      configurable: true,
      writable: true,
      value: TestWebSocket,
    });
  });
}

const session = {
  id: SESSION_ID,
  machine_id: 'mach_transcript',
  provider: 'mock',
  agent_type: 'mock',
  status: 'running',
  cwd: '/tmp/mock',
  title: 'Transcript fixture',
  created_at: '2026-01-01T10:00:00Z',
  updated_at: '2026-01-01T10:00:00Z',
};

async function mockApi(page: Page) {
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
    if (path === '/api/v1/orgs') {
      await route.fulfill({ json: [] });
      return;
    }
    if (path === '/api/v1/realtime/ticket') {
      await route.fulfill({
        json: { ticket: 'test-ticket', org_id: 'org_default', expires_in: 60 },
      });
      return;
    }
    if (path === `/api/v1/sessions/${SESSION_ID}/permissions`) {
      await route.fulfill({ json: [] });
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
    if (path === '/api/v1/session-folders') {
      await route.fulfill({ json: [] });
      return;
    }
    if (path === '/api/v1/sessions') {
      // The rail sends paging parameters and expects the envelope; legacy
      // callers (top bar, command palette) still expect a bare array.
      if (url.searchParams.has('limit') || url.searchParams.has('sort')) {
        await route.fulfill({ json: { items: [session] } });
      } else {
        await route.fulfill({ json: [session] });
      }
      return;
    }
    if (path === '/api/v1/machines') {
      await route.fulfill({ json: [] });
      return;
    }
    await route.fulfill({
      status: 404,
      json: { code: 'not_found', message: `unmocked request: ${path}` },
    });
  });
}

interface EmitOptions {
  turnId?: string;
  messageId?: string;
  timestamp?: string;
}

async function emit(page: Page, type: string, payload: unknown, options: EmitOptions = {}) {
  eventSequence += 1;
  const sequence = eventSequence;
  await page.evaluate(
    ({ sequence: seq, type: eventType, payload: body, options: extras }) => {
      const socket = window.__fleetSockets.at(-1);
      if (!socket?.onmessage) throw new Error('the event socket is not open');
      const agentEvent = {
        event_id: `evt_${seq}`,
        session_id: 'sess_transcript',
        sequence: seq,
        type: eventType,
        timestamp: extras.timestamp ?? new Date(Date.UTC(2026, 0, 1, 10, 0, seq)).toISOString(),
        provider: 'mock',
        payload: body,
        ...(extras.turnId ? { turn_id: extras.turnId } : {}),
        ...(extras.messageId ? { message_id: extras.messageId } : {}),
      };
      socket.onmessage({
        data: JSON.stringify({
          event_id: `platform_${seq}`,
          sequence: seq,
          type: 'agent.event',
          timestamp: agentEvent.timestamp,
          session_id: agentEvent.session_id,
          payload: agentEvent,
        }),
      });
    },
    { sequence, type, payload, options },
  );
}

async function openSession(page: Page) {
  await page.goto(`/sessions/${SESSION_ID}`);
  // The event socket opens asynchronously after the ticket round-trip.
  await expect.poll(() => page.evaluate(() => window.__fleetSockets.length)).toBeGreaterThan(0);
  await emit(page, 'session.created', { session });
  await expect(page.getByRole('heading', { name: 'Transcript fixture' })).toBeVisible();
  await expect(page.getByRole('log', { name: 'Session transcript' })).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(
    ([key, token]) => window.localStorage.setItem(key, token),
    [TOKEN_STORAGE_KEY, USER_TOKEN],
  );
});

test('streams a reply into a growing row and closes it with a turn status', async ({ page }) => {
  await installFakeWebSocket(page);
  await mockApi(page);
  await openSession(page);

  await emit(page, 'turn.started', {}, { turnId: 'turn_1', timestamp: '2026-01-01T10:00:00Z' });
  await emit(
    page,
    'message.started',
    { message_id: 'msg_1', role: 'assistant' },
    { turnId: 'turn_1', messageId: 'msg_1' },
  );

  const transcript = page.getByRole('log', { name: 'Session transcript' });
  await emit(
    page,
    'message.delta',
    { message_id: 'msg_1', text: 'Hello' },
    { turnId: 'turn_1', messageId: 'msg_1' },
  );
  await expect(transcript).toContainText('Hello');

  // A later delta extends the same row rather than adding a second one.
  await emit(
    page,
    'message.delta',
    { message_id: 'msg_1', text: ', streaming world' },
    { turnId: 'turn_1', messageId: 'msg_1' },
  );
  await expect(transcript.getByText('Hello, streaming world')).toHaveCount(1);

  await emit(page, 'turn.completed', {}, { turnId: 'turn_1', timestamp: '2026-01-01T10:01:00Z' });
  const status = page.getByTestId('turn-status');
  await expect(status).toContainText('Completed');
  await expect(status).toContainText('1m 0s');
});

test('renders notices, a plan card and an inline tool line', async ({ page }) => {
  await installFakeWebSocket(page);
  await mockApi(page);
  await openSession(page);

  await emit(page, 'agent.notice', { kind: 'retry', level: 'warning', message: 'Retrying…' });
  await emit(page, 'agent.plan.updated', {
    plan: {
      id: 'plan_1',
      session_id: SESSION_ID,
      entries: [
        { content: 'Read the failing test', status: 'completed', priority: 'high' },
        { content: 'Fix the reducer', status: 'in_progress', priority: 'high' },
      ],
      updated_at: '2026-01-01T10:00:30Z',
    },
  });
  await emit(
    page,
    'tool.started',
    {
      tool: {
        id: 'tool_1',
        session_id: SESSION_ID,
        name: 'Bash',
        status: 'running',
        input: { command: 'cargo test -p web' },
        started_at: '2026-01-01T10:00:40Z',
      },
    },
    { timestamp: '2026-01-01T10:00:40Z' },
  );

  await expect(page.getByText('Retrying…')).toBeVisible();
  const plan = page.getByTestId('plan-card');
  await expect(plan).toContainText('Fix the reducer');
  await expect(plan).toContainText('1/2');

  const tool = page.getByTestId('tool-card').filter({ hasText: 'Bash' });
  await expect(tool).toBeVisible();
  await expect(tool).toContainText('Ran cargo test -p web');
});

test('keeps an unpinned transcript in place and offers jump to latest', async ({ page }) => {
  await installFakeWebSocket(page);
  await mockApi(page);
  await openSession(page);

  // Enough rows to overflow the viewport.
  for (let index = 0; index < 12; index += 1) {
    await emit(page, 'message.completed', {
      message: {
        id: `msg_${index}`,
        session_id: SESSION_ID,
        role: 'user',
        content: `Line ${index}\n${'padding '.repeat(20)}`,
        created_at: '2026-01-01T10:00:00Z',
      },
    });
  }

  const transcript = page.getByRole('log', { name: 'Session transcript' });
  const scroller = transcript;
  await expect(page.getByText('Line 11', { exact: false })).toBeVisible();

  // Scroll away from the bottom; the view must latch to unpinned.
  await transcript.hover();
  await page.mouse.wheel(0, -2000);
  await expect(page.getByRole('button', { name: 'Jump to latest' })).toBeVisible();
  const offset = await scroller.evaluate((element) => element.scrollTop);
  const distanceFromBottom = await scroller.evaluate(
    (element) => element.scrollHeight - element.scrollTop - element.clientHeight,
  );
  expect(distanceFromBottom).toBeGreaterThan(80);

  // New output must not yank the viewport back down.
  await emit(page, 'message.completed', {
    message: {
      id: 'msg_new',
      session_id: SESSION_ID,
      role: 'assistant',
      content: `New output\n${'padding '.repeat(20)}`,
      created_at: '2026-01-01T10:00:59Z',
    },
  });
  await expect(page.getByText('New output', { exact: false })).toHaveCount(1);
  const afterOffset = await scroller.evaluate((element) => element.scrollTop);
  expect(Math.abs(afterOffset - offset)).toBeLessThan(4);

  // Jumping back pins to the bottom.
  await page.getByRole('button', { name: 'Jump to latest' }).click();
  await expect(page.getByRole('button', { name: 'Jump to latest' })).toHaveCount(0);
  const distance = await scroller.evaluate(
    (element) => element.scrollHeight - element.scrollTop - element.clientHeight,
  );
  expect(distance).toBeLessThan(4);
});

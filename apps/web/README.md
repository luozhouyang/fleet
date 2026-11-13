# Agent Fleet Web UI

Vite + React 19 operations console for machines, workspaces, Git changes and
coding-agent sessions.

## Run

From the repository root:

```bash
pnpm install
pnpm --filter @agent-fleet/web dev
```

The app runs on <http://localhost:5173>. Start the control plane separately on
`127.0.0.1:8787`.

The Vite dev server proxies `/api` and `/ws` to `http://127.0.0.1:8787`, with
WebSocket support enabled, so browser REST calls and `/api/v1/events` work
without CORS setup.

## Auth token

The token is stored in `localStorage` under `agent-fleet.api-token` and defaults
to `dev-token`. REST requests send `Authorization: Bearer <token>`. Browsers
cannot set custom headers on a WebSocket, so the event client appends the token
as `?token=...` to `/api/v1/events`.

Use the token control in the top bar to change it; the event socket reconnects
immediately.

## Build and checks

```bash
pnpm --filter @agent-fleet/web build
pnpm --filter @agent-fleet/web exec tsc -b
pnpm --filter @agent-fleet/web lint
pnpm --filter @agent-fleet/web format
```

`build` runs TypeScript project builds and then Vite; output is written to
`apps/web/dist`.

## Layout

- `src/features/machines` — machine fleet and machine detail
- `src/features/workspaces` — lazy tree, read-only Monaco viewer, Git diff panel
- `src/features/sessions` — session list and streaming chat timeline
- `src/lib` — API/event clients, query keys and hooks, normalized session store
- `../../packages/protocol-types` — TypeScript mirror of `crates/protocol`
- `../../packages/api-client` — typed REST and WebSocket clients
- `../../packages/ui` — small local Tailwind component set

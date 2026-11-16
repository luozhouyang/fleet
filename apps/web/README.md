# Fleet Web

Session-first workbench for remote coding agents.

## Stack

- React 19 + TypeScript
- TanStack Router (code-based routes) and TanStack Query
- Tailwind CSS v4 with design tokens in `src/styles.css`
- Base UI primitives (`@base-ui-components/react`) wrapped in `src/components/ui`
- Monaco for file and diff viewing
- Zustand for the live session event store

## Layout

```
src/
  app/          router + providers
  components/
    ui/         design-system primitives (button, input, dialog, tabs, select…)
    layout/     top bar and app shell
    dialogs/    shared new-session / add-workspace / confirm dialogs
    editor/     Monaco code + diff editors
    status/     status dot indicators
  features/
    sessions/   session workbench (list / conversation / context panel)
    workspaces/ workspace ledger and editor
    machines/   machine ledger and detail
  lib/
    api/        API + event clients (token-aware)
    query/      query keys, hooks, mutations
    events/     event socket bridge and connection provider
    session-*   reducer/store/timeline for streamed agent events
    utils/      formatting, language detection, cn()
```

## Conventions

- Status is always a 6px dot plus a status-colored label. Never add decorative
  left color rails to rows or cards.
- Use the semantic token classes (`bg-card`, `border-border`, `text-muted-foreground`,
  `bg-primary-soft`, `text-success`, …) instead of raw colors.
- Numeric data uses `font-mono` and tabular numerals.
- Surface depth comes from layered backgrounds and `shadow-card` / `shadow-modal`,
  not from borders on every element.
- Prefer the shared primitives in `components/ui` over ad-hoc controls.

## Commands

```bash
pnpm --filter @agent-fleet/web dev
pnpm --filter @agent-fleet/web build
pnpm --filter @agent-fleet/web exec tsc -p tsconfig.app.json --noEmit
pnpm --filter @agent-fleet/web lint
pnpm --filter @agent-fleet/web test:e2e
```

The dev server proxies `/api` and `/ws` to `127.0.0.1:8787` (agent admin).

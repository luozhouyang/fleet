# agent-fleet

Remote Coding Agent Management Platform — manage coding agents running on your
own machines from one web UI.

- **Architecture:** [`docs/architecture.md`](docs/architecture.md)
- **Implementation plan:** [`docs/implementation.md`](docs/implementation.md)
- **Frozen crate contracts:** [`docs/crate-interfaces.md`](docs/crate-interfaces.md)
- **Wire protocols:** [`docs/protocols.md`](docs/protocols.md)
- **Security model:** [`docs/security.md`](docs/security.md)
- **Development guide:** [`docs/development.md`](docs/development.md)

## What works (MVP)

- **Machine daemon** (Rust) with an Ed25519 identity, heartbeat, exponential
  backoff reconnect and process-group cleanup.
- **Reverse WebSocket connection** with token auth, challenge/response machine
  auth, request/response correlation, timeouts and server-side liveness checks.
- **Control plane** (Rust/Axum): machines, workspaces, files/tree/search, Git
  status/diff/branches/log, agent sessions, permissions, browser event stream,
  bearer auth and SPA static serving.
- **Agent runtime**: unified session/turn/message/tool/permission/event model
  with five adapters — `mock` (deterministic, used by E2E), Claude Code CLI
  (`stream-json`), Codex app-server (JSON-RPC), OpenCode server (HTTP + SSE)
  and a generic PTY adapter.
- **Workspace service**: lazy directory listing, ignore rules, ETag reads,
  binary/truncation handling, search, path-traversal/symlink protection and a
  debounced file watcher that keeps Git status fresh.
- **Git service**: porcelain v2 status, unified diff parser, untracked-file
  diffs, summaries, branches and log — Git CLI with argv-only invocation.
- **Web UI** (React 19 + TanStack Router/Query + Tailwind + Monaco): machine
  dashboard, workspace explorer, read-only Monaco viewer, Git changes and diff
  viewer, agent chat with streaming deltas, tool-call cards, permission
  approval and a reconnecting event WebSocket.
- **Tests**: ~250 Rust tests (unit + integration + loopback E2E), strict
  TypeScript, ESLint, and a black-box smoke script.

## Layout

```
apps/agent-daemon      local machine daemon (agents + workspace + git)
apps/control-plane     remote control plane (REST + WebSocket)
apps/web               React web UI
crates/*               shared Rust libraries
packages/*             shared TypeScript packages
migrations/schema      SQLite migrations
scripts/test           black-box E2E smoke test
```

## Toolchain

This project builds with the prebuilt cargo at:

```text
/home/luozhouyang/Applications/pcargo/cargo
```

The Makefile defaults to it; override with `PCARGO=/path/to/cargo`.

```bash
make build          # cargo build --workspace
make test           # cargo test --workspace + web checks
make lint           # clippy -D warnings + eslint
```

## Quick start (development)

```bash
# 1. control plane
AGENT_FLEET_ADMIN_TOKEN=dev-token make dev-control-plane   # 127.0.0.1:8787

# 2. daemon (new terminal)
AGENT_FLEET_SERVER_URL=ws://127.0.0.1:8787/ws/machine \
AGENT_FLEET_TOKEN=dev-token \
AGENT_FLEET_MACHINE_NAME=dev-machine \
make dev-daemon                                            # 127.0.0.1:18080

# 3. web UI (new terminal)
make web-install && make dev-web                           # 127.0.0.1:5173
```

Then open the UI, pick the machine, register a workspace, browse files and Git
changes, and create a session with the `mock` provider (no external quota) or
`claude` / `codex` / `opencode` when those tools are installed. The daemon
starts a private `opencode serve` per machine unless `OPENCODE_SERVER_URL`
points at an existing server.

The control plane serves `apps/web/dist` automatically when present, so
`make web-build` + the control plane is enough for a single-binary deployment.

## End-to-end smoke test

Starts a real control plane + daemon against a temporary Git repository and
exercises registration, providers, workspace tree/file/ETag, Git status/diff,
streaming chat, tool permission approval, interrupt and cleanup:

```bash
./scripts/test/e2e_smoke.sh        # add E2E_KEEP=1 to keep artifacts
```

Requires `curl`, `jq` and `git`.

## Providers

| Provider | Integration | Capabilities |
|---|---|---|
| `mock` | in-process | streaming, tools, permissions, interrupt, resume |
| `claude` | `claude -p --input-format stream-json --output-format stream-json --verbose` | streaming, tools, interrupt, resume; no interactive permission prompts |
| `codex` | `codex app-server --listen stdio://` (JSON-RPC 2.0) | streaming, tools, approvals, interrupt, resume |
| `opencode` | `opencode serve` v2 HTTP API + `GET /api/event` SSE | streaming, tools, approvals, interrupt, resume, session persistence |
| `generic` | PTY | raw terminal streaming, interrupt |

Claude/Codex adapters are verified with anonymized protocol fixtures, and the
OpenCode adapter has a full mock-server integration suite. One ignored test
(`real_server_creates_and_deletes_a_session`) exercises a real `opencode serve`
without sending prompts (no model quota). Refresh fixtures when upgrading.

## Known limitations (MVP)

- Single shared admin token; no multi-user/org ACLs.
- Machine pairing is trust-on-first-use gated by that token (no approval UI).
- Agent processes are not sandboxed by the daemon.
- Git operations are read-only (no stage/commit/checkout).
- File viewing is read-only; no remote editing.
- Multi-instance control planes / Redis / NATS routing are not implemented.
- OpenCode supports text streaming, tools and permissions; structured diffs and
  image attachments are not mapped yet.

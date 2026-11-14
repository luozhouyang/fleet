# agent-fleet

Remote Coding Agent Management Platform — manage coding agents running on your
own machines from one web UI.

- **Architecture:** [`docs/architecture.md`](docs/architecture.md)
- **Implementation plan:** [`docs/implementation.md`](docs/implementation.md)
- **Frozen crate contracts:** [`docs/crate-interfaces.md`](docs/crate-interfaces.md)
- **Wire protocols:** [`docs/protocols.md`](docs/protocols.md)
- **Security model:** [`docs/security.md`](docs/security.md)
- **Connectivity & enrollment:** [`docs/connectivity.md`](docs/connectivity.md)
- **Development guide:** [`docs/development.md`](docs/development.md)

## What works (MVP)

- **Machine daemon** (Rust) with an Ed25519 identity, heartbeat, exponential
  backoff reconnect and process-group cleanup.
- **Reverse WebSocket connection** with token auth, challenge/response machine
  auth, proof-of-possession key rotation, request/response correlation,
  timeouts and server-side liveness checks.
- **Organizations and per-user tokens** with admin/member/viewer roles,
  organization-scoped machines, workspaces, sessions and audit records.
- **Git writes** (stage, unstage, commit, checkout) behind an independent
  write token and an append-only audit log.
- **Agent Admin** (Rust/Axum): machines, workspaces, files/tree/search, Git
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
- **Tests**: 286 Rust tests (unit + integration + loopback E2E), strict
  TypeScript, ESLint, and a black-box smoke script.

## Layout

```
apps/agent-daemon      local machine daemon (agents + workspace + git)
apps/agent-admin       agent admin (REST + WebSocket + serves the web UI)
apps/agent-cli         `af` management CLI (REST client + network wizard)
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

The root `package.json` scripts default to it; override with `PCARGO=/path/to/cargo`.

```bash
pnpm build:rust     # cargo build --workspace
pnpm test           # cargo test --workspace + web checks
pnpm lint           # clippy -D warnings + eslint
```

## Quick start (development)

Development also runs as a single web-serving process: `pnpm dev` builds the
web UI and starts the agent admin, which serves both `/api` and the SPA.

```bash
pnpm web:install

# 1. agent admin: builds the UI, then serves API + SPA on 127.0.0.1:8787
pnpm dev

# 2. daemon (new terminal)
AGENT_FLEET_SERVER_URL=ws://127.0.0.1:8787/ws/machine \
AGENT_FLEET_TOKEN=dev-machine-token \
AGENT_FLEET_MACHINE_NAME=dev-machine \
pnpm dev:daemon                                            # 127.0.0.1:18080
```

Open **http://127.0.0.1:8787**, pick the machine, register a workspace, browse
files and Git changes, and create a session with the `mock` provider (no
external quota) or `claude` / `codex` / `opencode` when those tools are
installed. The daemon starts a private `opencode serve` per machine unless
`OPENCODE_SERVER_URL` points at an existing server.

`pnpm dev` defaults to `AGENT_FLEET_ADMIN_TOKEN=dev-token` and
`AGENT_FLEET_MACHINE_TOKEN=dev-machine-token`; export either variable to
override them. For frontend hot reload you can optionally run
`pnpm dev:web` (Vite on 5173, proxying `/api` and `/ws` to 8787) instead of
using the SPA served by the admin.

## Management CLI (`af`)

`af` is a thin REST client for the agent admin, plus a guided remote-access
setup for Tailscale and Cloudflare Tunnel:

```bash
pnpm build:cli                 # builds target/debug/af
pnpm run af -- --help           # or run ./target/debug/af directly

./target/debug/af --url http://127.0.0.1:8787 --token dev-token machines list
./target/debug/af users create --org org_default --name Ada --role member
./target/debug/af users rotate-token user_...
./target/debug/af workspaces add --machine m_... --root /path/to/repo
./target/debug/af sessions list
./target/debug/af audit --limit 50

# write ~/.agent-fleet/cli.toml so --url/--token are optional afterwards
./target/debug/af config init --url http://127.0.0.1:8787 --token dev-token

# inspect Tailscale/cloudflared and generate admin/daemon/tunnel config
./target/debug/af network detect
./target/debug/af network wizard --mode tailscale
./target/debug/af network wizard --mode cloudflare --hostname fleet.example.com

# preview an authorized end-to-end setup (no external changes)
./target/debug/af network connect --provider tailscale
./target/debug/af network connect --provider cloudflare --hostname fleet.example.com

# authorize, apply and verify (each step confirmed unless --yes)
./target/debug/af network connect --provider tailscale --execute
./target/debug/af network connect --provider cloudflare --hostname fleet.example.com \
  --execute --access-email you@example.com
```

`network connect` uses the vendors' official browser authorization
(`tailscale up`, `cloudflared tunnel login`), then creates/reuses the tunnel,
writes DNS, installs the service, writes admin/daemon/CLI config with backups,
optionally creates a Cloudflare Access application, and verifies the public
health endpoint. External and privileged commands only run with `--execute`
and are confirmed individually unless `--yes` is passed. Without `--execute`
it prints a preview and writes nothing.

Cloudflare Access needs a scoped API token (`Access: Apps and Policies:Edit`,
`Access: Organizations:Read`, `Account Settings:Read`) passed with
`--cloudflare-api-token` or `CLOUDFLARE_API_TOKEN`; it is stored in the OS
keyring with a 0600 fallback file. Pass `--machine-token` (or set
`AGENT_FLEET_MACHINE_TOKEN`) to also write the daemon `connection.token`.

On macOS, add `--install` to let the CLI offer `brew install cloudflared` or
`brew install --cask tailscale`. Tailscale still requires opening the app once
and approving the VPN/network-extension prompts; those cannot be automated.

## Reproducible Linux verification (Docker)

Run the same checks on a clean Linux image without touching the host toolchain:

```bash
pnpm docker:verify:quick   # fmt, clippy, Rust tests, web typecheck/lint/build
pnpm docker:verify         # + API/Git/multi-machine E2E + Playwright UI E2E
pnpm docker:verify:fresh   # same as above, rebuilding layers and skipping caches
```

The image (`docker/verify.Dockerfile`) contains Rust, Node/pnpm and Playwright
Chromium; E2E stacks bind loopback inside the container, so no ports are
published. By default the cargo registry and `target` directories are kept in
named Docker volumes for faster rebuilds; use `fresh` for a cache-free run.

## End-to-end smoke test

Starts a real agent admin + daemon against a temporary Git repository and
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

- Users belong to exactly one organization; no cross-org guests or per-file ACLs.
- Tokens have no expiry; rotate or delete a user to revoke access.
- Machine pairing is trust-on-first-use gated by the daemon token (no approval UI).
- Agent processes are not sandboxed by the daemon.
- Git merges, rebases, pushes and history rewriting are not exposed.
- File viewing is read-only; no remote editing.
- Multi-instance agent admins / Redis / NATS routing are not implemented.
- OpenCode supports text streaming, tools and permissions; structured diffs and
  image attachments are not mapped yet.

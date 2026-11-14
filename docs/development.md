# Development Guide

## Toolchain

Always build with the prebuilt cargo binary:

```bash
/home/luozhouyang/Applications/pcargo/cargo
```

Examples:

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo check --workspace
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test --workspace
```

The root `package.json` scripts use this binary by default (`PCARGO`).
Node/pnpm 10 is used for the web workspace.

## Layout and dependency rules

```
common  <-  protocol  <-  {process-manager, storage, auth, event-bus, workspace,
                           git-service, agent-runtime, connection-gateway,
                           machine-registry}  <-  {agent-daemon, agent-admin}
```

- `protocol` contains DTOs only; it never performs I/O.
- `agent-runtime` never depends on `workspace` or `git-service`.
- `workspace` and `git-service` never depend on each other.
- `connection-gateway` never depends on a concrete provider adapter.
- Only `storage` opens SQLite. Only `git-service` runs `git`. Only
  `agent-runtime` spawns agent processes.

## Local development

Development uses one web-serving process: `pnpm dev` builds `apps/web/dist`
and starts the agent admin, which serves the API and the SPA on port 8787.

Terminal 1 — agent admin + web UI:

```bash
pnpm web:install
pnpm dev              # http://127.0.0.1:8787
```

`pnpm dev` defaults to the development admin token `dev-token` and machine
token `dev-machine-token`; export `AGENT_FLEET_ADMIN_TOKEN` /
`AGENT_FLEET_MACHINE_TOKEN` to override them.

Terminal 2 — daemon:

```bash
AGENT_FLEET_SERVER_URL=ws://127.0.0.1:8787/ws/machine \
AGENT_FLEET_TOKEN=dev-machine-token \
AGENT_FLEET_MACHINE_NAME=dev-machine \
pnpm dev:daemon
```

Optional frontend hot reload: `pnpm dev:web` runs Vite on
`http://127.0.0.1:5173` and proxies `/api` and `/ws` to 8787. The default
`pnpm dev` path has no HMR because the SPA is served from `dist`.

The default database moved from `~/.agent-fleet/control-plane.db` to
`~/.agent-fleet/agent-admin.db`. Existing deployments can rename the file or
pass `--db /path/to/control-plane.db` to keep the old location.

## Management CLI (`af`)

`apps/agent-cli` builds `af`, a thin admin REST client with a remote-access
wizard. Build it with `pnpm build:cli` and use `target/debug/af`, or run it through
pnpm with `pnpm run af -- <command>`.

```bash
af config init --url http://127.0.0.1:8787 --token dev-token
af health
af machines list
af workspaces list --machine m_...
af sessions list
af users create --org org_default --name Ada --role member
af users rotate-token user_...
af audit --limit 50

af network detect
af network wizard --mode tailscale --yes
af network wizard --mode cloudflare --hostname fleet.example.com

# preview / execute the authorized one-shot setup
af network connect --provider tailscale
af network connect --provider cloudflare --hostname fleet.example.com \
  --execute --access-email you@example.com
```

Global flags are `--url`, `--token`, `--config` and `--json`; the URL/token
also come from `AGENT_FLEET_ADMIN_URL` / `AGENT_FLEET_ADMIN_TOKEN` or
`~/.agent-fleet/cli.toml`. The network wizard only writes configuration
(admin `server.public_url`, daemon `connection.server_url`, CLI `public_url`,
and the Cloudflare tunnel YAML when the tunnel already exists). Pass
`--machine-token` or set `AGENT_FLEET_MACHINE_TOKEN` to write the daemon
`connection.token` as well. The wizard prints the `tailscale serve` /
`cloudflared tunnel` commands instead of executing them.

`network connect` upgrades this to the vendors' browser authorization
(`tailscale up`, `cloudflared tunnel login`). External and privileged commands
run only with `--execute`, and are confirmed one by one unless `--yes` is
passed. It can also create a Cloudflare Access app/policy
(`--access-email`, `--access-domain`, `--access-everyone`) using a scoped API
token stored in the OS keyring (0600 fallback file), and install cloudflared as
a system service on Linux. Use `--skip-access` / `--skip-service` to opt out.

## Container verification

`docker/verify.Dockerfile` builds a clean Debian image with Rust, Node/pnpm and
Playwright Chromium. The E2E scripts start the admin/daemon inside the
container and bind loopback only; no ports are published.

```bash
pnpm docker:verify:quick   # fmt, clippy, tests, web checks
pnpm docker:verify         # + API, Git write, multi-machine and UI E2E
pnpm docker:verify:fresh   # --no-cache build, no cargo registry/target volumes
```

Notes:

- The image defaults to `rust:1.97.1-bookworm` (any Rust >= 1.85 works) and Node 22.
- Named volumes cache `/usr/local/cargo/registry` and `/workspace/target`.
  They only cache dependencies/build artifacts, never source changes.
- Use this for Linux reproducibility. It cannot validate macOS-only behaviour
  (Homebrew installs, Tailscale app permissions, launchd services) or real
  agent providers; those still need a host run with the mock provider as a
  baseline.
- Avoid bind-mounting the repository from macOS into the container for file
  watching tests: inotify over virtiofs/9p is unreliable and slow. The image
  copies the tree and runs from container-local storage.

## Database migrations

`migrations/schema` is append-only: SQLx stores a SHA-384 checksum in
`_sqlx_migrations`, so editing an already-applied file makes existing daemon or
admin databases fail to start. Add a new numbered migration instead of changing
an old one (even comments). The separate daemon/admin databases use the same
schema and both validate those checksums.

## Provider smoke checks

The default provider is `mock`, which needs no credentials and is used by the
E2E tests. Real providers:

- Claude Code CLI: `claude -p --input-format stream-json --output-format stream-json --verbose`
- Codex app server: `codex app-server --listen stdio://`
- OpenCode: the daemon starts `opencode serve --hostname 127.0.0.1 --port 0`
  and reads the printed URL/password; set `OPENCODE_SERVER_URL` (and optionally
  `OPENCODE_SERVER_PASSWORD`) to reuse an existing v2 server instead.

Adapter protocol fixtures live next to the adapters in
`crates/agent-runtime/src/<provider>/fixtures/` and are refreshed whenever the
upstream CLI changes. Tests must not call the real CLIs.

## Users, organizations and machine key rotation

The bootstrap admin token remains the super-admin. Create organizations and
users through the API (user tokens are returned exactly once):

```bash
curl -X POST -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
  -d '{"name":"Acme"}' http://127.0.0.1:8787/api/v1/orgs
curl -X POST -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
  -d '{"org_id":"org_default","name":"Ada","role":"member"}' \
  http://127.0.0.1:8787/api/v1/users
curl -X POST -H "Authorization: Bearer $ADMIN" \
  http://127.0.0.1:8787/api/v1/users/user_.../rotate-token
```

Roles are `admin`, `member` and `viewer` (read-only). Machines are assigned to
`org_default` on first connection; move one with
`PUT /api/v1/machines/{id}/org`. Git write policy is controlled by `[git]
allow_writes` / `AGENT_FLEET_GIT_WRITES` and the optional
`git.write_token` / `AGENT_FLEET_GIT_WRITE_TOKEN` (browser equivalent: the
token dialog's Git write token field).

Rotate a machine key while its daemon is stopped:

```bash
agent-fleet-daemon --config /path/to/config.toml rotate-key
agent-fleet-daemon --config /path/to/config.toml serve
```

The stable machine id lives in `<key_file>.id`; the old key is kept in
`<key_file>.previous` only until the agent admin accepts the rotation proof.

## Data directories

- Daemon: `~/.agent-fleet/agent.db`, `~/.agent-fleet/machine.key`
- Agent Admin: `~/.agent-fleet/agent-admin.db`

Delete these for a clean slate (machines must re-register).

## Testing

- Rust unit tests live in each crate.
- Cross-crate integration tests live in `tests/`.
- The black-box API smoke test uses the `mock` provider and needs no network:
  `./scripts/test/e2e_smoke.sh` (`pnpm e2e`).
- The browser E2E suite drives Chromium against a live stack (agent admin +
  daemon + a temporary git repository) and covers the machine dashboard, lazy
  workspace tree, Monaco file/diff viewers, file-change warning, session
  creation and streaming, permission approval, interrupt and close, plus Git
  staging, commit, branch checkout and the audit trail:

```bash
pnpm --filter @agent-fleet/web exec playwright install chromium   # once
pnpm e2e:ui                                                       # scripts/test/ui_e2e.sh
```

- The Git write test exercises the dedicated write token, stage/unstage,
  path-limited commit, conflict handling, branch checkout and the audit trail:
  `./scripts/test/git_write_e2e.sh` (`pnpm e2e:git`).
- The multi-machine stress test runs one agent admin with three daemons and
  six git workspaces. It checks concurrent tree/file reads, session streaming,
  permission approvals, interrupts, cross-session isolation, an offline machine,
  and workspace recovery after a daemon restart:

```bash
pnpm e2e:multi   # scripts/test/multi_machine_stress.sh
```

- Run everything: `pnpm test` (Rust + web typecheck/lint; the suites above are
  separate because they spawn servers and browsers).

## Security notes for development

- There is no built-in default credential. Configure `AGENT_FLEET_ADMIN_TOKEN`
  (or `--admin-token`) and, preferably, a distinct
  `AGENT_FLEET_MACHINE_TOKEN` for the machine WebSocket. If the machine token is
  omitted the admin token is accepted as a backward-compatible fallback and a
  startup warning is logged.
- Sessions should pass `workspace_id`. A bare `cwd` is only accepted when it
  canonicalizes inside a workspace already registered on that machine.
- Machine identity is an Ed25519 key pair. The agent admin pins the first
  public key it sees per machine id; key changes are rejected until the machine
  is removed.
- Every workspace path is resolved by `PathPolicy`; `..`, absolute paths and
  symlinks escaping the workspace root are denied.

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

The `Makefile` uses this binary by default (`PCARGO`). Node/pnpm 10 is used for
the web workspace.

## Layout and dependency rules

```
common  <-  protocol  <-  {process-manager, storage, auth, event-bus, workspace,
                           git-service, agent-runtime, connection-gateway,
                           machine-registry}  <-  {agent-daemon, control-plane}
```

- `protocol` contains DTOs only; it never performs I/O.
- `agent-runtime` never depends on `workspace` or `git-service`.
- `workspace` and `git-service` never depend on each other.
- `connection-gateway` never depends on a concrete provider adapter.
- Only `storage` opens SQLite. Only `git-service` runs `git`. Only
  `agent-runtime` spawns agent processes.

## Local development

Terminal 1 — control plane:

```bash
AGENT_FLEET_ADMIN_TOKEN=dev-token make dev-control-plane
```

Terminal 2 — daemon:

```bash
AGENT_FLEET_SERVER_URL=ws://127.0.0.1:8787/ws/machine \
AGENT_FLEET_TOKEN=dev-token \
AGENT_FLEET_MACHINE_NAME=dev-machine \
make dev-daemon
```

Terminal 3 — web UI:

```bash
make web-install
make dev-web          # http://127.0.0.1:5173, proxies /api to :8787
```

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

## Data directories

- Daemon: `~/.agent-fleet/agent.db`, `~/.agent-fleet/machine.key`
- Control plane: `~/.agent-fleet/control-plane.db`

Delete these for a clean slate (machines must re-register).

## Testing

- Rust unit tests live in each crate.
- Cross-crate integration tests live in `tests/`.
- The black-box API smoke test uses the `mock` provider and needs no network:
  `./scripts/test/e2e_smoke.sh` (`make e2e`).
- The browser E2E suite drives Chromium against a live stack (control plane +
  daemon + a temporary git repository) and covers the machine dashboard, lazy
  workspace tree, Monaco file/diff viewers, file-change warning, session
  creation and streaming, permission approval, interrupt and close, plus Git
  staging, commit, branch checkout and the audit trail:

```bash
pnpm --filter @agent-fleet/web exec playwright install chromium   # once
make e2e-ui                                                       # scripts/test/ui_e2e.sh
```

- The Git write test exercises the dedicated write token, stage/unstage,
  path-limited commit, conflict handling, branch checkout and the audit trail:
  `./scripts/test/git_write_e2e.sh` (`make e2e-git`).
- The multi-machine stress test runs one control plane with three daemons and
  six git workspaces. It checks concurrent tree/file reads, session streaming,
  permission approvals, interrupts, cross-session isolation, an offline machine,
  and workspace recovery after a daemon restart:

```bash
make e2e-multi   # scripts/test/multi_machine_stress.sh
```

- Run everything: `make test` (Rust + web typecheck/build; the suites above are
  separate because they spawn servers and browsers).

## Security notes for development

- The MVP uses a single-user bearer token. `dev-token` is only a default for
  local development when `AGENT_FLEET_DEV=1`; set an explicit token otherwise.
- Machine identity is an Ed25519 key pair. The control plane pins the first
  public key it sees per machine id; key changes are rejected until the machine
  is removed.
- Every workspace path is resolved by `PathPolicy`; `..`, absolute paths and
  symlinks escaping the workspace root are denied.

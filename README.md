# agent-fleet

Remote Coding Agent Management Platform.

- **Architecture:** [`docs/architecture.md`](docs/architecture.md)
- **Implementation plan:** [`docs/implementation.md`](docs/implementation.md)
- **Crate interfaces (frozen for MVP):** [`docs/crate-interfaces.md`](docs/crate-interfaces.md)

## Layout

```
apps/agent-daemon      local machine daemon (agents + workspace + git)
apps/control-plane     remote control plane (REST + WebSocket)
apps/web               React web UI
crates/*               shared Rust libraries
packages/*             shared TypeScript packages
migrations/schema      SQLite migrations
```

## Toolchain

This project builds with the prebuilt cargo at:

```text
/home/luozhouyang/Applications/pcargo/cargo
```

Use it explicitly (the Makefile defaults to it):

```bash
/home/luozhouyang/Applications/pcargo/cargo build
/home/luozhouyang/Applications/pcargo/cargo test
```

Override with `PCARGO=/path/to/cargo make ...` when needed.

## Quick start (development)

```bash
make build                 # cargo build --workspace
make test                  # cargo test --workspace
make dev-control-plane     # 127.0.0.1:8787
make dev-daemon            # connects to control plane, serves 127.0.0.1:18080
make dev-web               # Vite dev server on 127.0.0.1:5173
```

Default single-user token for local development: `dev-token`.
Set `AGENT_FLEET_ADMIN_TOKEN` / `AGENT_FLEET_TOKEN` to change it.

## MVP flow

1. Start the control plane.
2. Start the daemon with `AGENT_FLEET_SERVER_URL=ws://127.0.0.1:8787/ws/machine`.
3. Open the web UI, pick the machine, register a workspace, browse files and git changes.
4. Create a session with provider `mock` (no external quota) or `claude` / `codex`.

Real Claude/Codex adapters are exercised by fixture tests; the `mock` provider is
used for deterministic end-to-end tests.

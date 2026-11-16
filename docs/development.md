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
core-common  <-  core-protocol  <-  {daemon-process, core-auth, daemon-workspace,
                                  daemon-git, daemon-runtime, core-connection,
                                  admin-machine, admin-storage, daemon-storage}
                                  <-  {fleet-daemon, fleet-admin}
core-protocol  <-  admin-eventbus  <-  fleet-admin
```

- `core-protocol` contains DTOs only; it never performs I/O.
- `daemon-runtime` never depends on `daemon-workspace` or `daemon-git`.
- `daemon-workspace` and `daemon-git` never depend on each other.
- `core-connection` never depends on a concrete provider adapter.
- `admin-eventbus` is admin-domain browser fan-out; the daemon does not depend on it.
- Only `admin-storage` and `daemon-storage` open SQLite. Only `daemon-git`
  runs `git`. Only `daemon-runtime` spawns agent processes.

## Local development

Development uses one web-serving process: `pnpm dev` builds `apps/web/dist`
and starts `fleet-admin`, which serves the API and the SPA on port 8787.

Terminal 1 — fleet-admin + web UI:

```bash
pnpm web:install
pnpm dev              # http://127.0.0.1:8787
```

`pnpm dev` defaults to the development admin token `dev-token`; export
`FLEET_ADMIN_TOKEN` to override it. `pnpm dev` and `pnpm dev:web` inject
`VITE_DEFAULT_API_TOKEN` from `FLEET_ADMIN_TOKEN` (default `dev-token`) so
the local SPA authenticates automatically. Production builds do not embed a
default token: if no token is stored or a request returns 401, the SPA shows an
auth gate.

Terminal 2 — pair the machine once, then start the daemon:

```bash
# creates a one-shot pairing code, writes ~/.fleet/daemon/config.toml
# (server_url, machine.id/name) and stores the per-machine token in
# ~/.fleet/daemon/machine.key.token (0600)
pnpm run fleet -- --admin-url http://127.0.0.1:8787 --admin-token dev-token daemon setup \
  --org org_default \
  --server ws://127.0.0.1:8787/ws/machine \
  --config ~/.fleet/daemon/config.toml \
  --name dev-machine

pnpm dev:daemon
```

`fleet daemon setup` must run while the admin is up. Re-running it with the same
key is safe; it creates a fresh code and token. There is no machine-token
environment variable to set.

Optional frontend hot reload: `pnpm dev:web` runs Vite on
`http://127.0.0.1:5173` and proxies `/api` and `/ws` to 8787. The default
`pnpm dev` path has no HMR because the SPA is served from `dist`.

This iteration has no backward compatibility; start from `~/.fleet`. The admin
database defaults to `~/.fleet/admin/fleet-admin.db` and the daemon database to
`~/.fleet/daemon/fleet-daemon.db`. Pre-rebrand configs and state are not
migrated; start fresh from `~/.fleet`.

## Management CLI (`fleet`)

`apps/fleet-cli` builds `fleet`, a thin admin REST client with a remote-access
wizard. Build it with `pnpm build:cli` and use `target/debug/fleet`, or run it
through pnpm with `pnpm run fleet -- <command>`.

```bash
# interactive dispatcher; prompts for admin, daemon, or CLI
fleet setup

# scripted role setup (writes TOML + CLI config, optional systemd unit)
fleet admin setup --yes
fleet admin setup --user --no-web --dry-run
fleet daemon setup --server ws://127.0.0.1:8787/ws/machine --name dev-machine

# lifecycle (systemd when installed, otherwise the detached supervisor)
fleet admin start
fleet admin status
fleet admin logs -n 100
fleet admin restart --mode supervisor
fleet admin stop
fleet daemon start --mode supervisor

# both-role summary and read-only diagnostics
fleet status
fleet doctor

# service units and foreground runners
fleet admin install-service --dry-run
fleet admin uninstall-service --yes
fleet admin run
fleet daemon run

# inspect and validate role config; local daemon helpers
fleet admin config show
fleet daemon config validate
fleet daemon identity show
fleet daemon providers
fleet daemon workspace add /path/to/repo --name demo
fleet daemon workspace list
fleet daemon rotate-key

fleet config init --admin-url http://127.0.0.1:8787 --admin-token dev-token
fleet admin health
fleet admin machines list
fleet admin machines revoked
fleet admin machines unrevoke m_...
fleet admin workspaces list --machine m_...
fleet admin sessions list
fleet admin sessions close session_...
fleet admin sessions delete session_... --yes
fleet admin users create --org org_default --name Ada --role member
fleet admin users rotate-token user_...
fleet admin audit --limit 50

# enroll this machine (creates a code, pairs, writes config and token)
fleet --admin-url http://127.0.0.1:8787 --admin-token dev-token daemon setup \
  --org org_default \
  --server ws://127.0.0.1:8787/ws/machine \
  --config ~/.fleet/daemon/config.toml \
  --name dev-machine
# split flow: create the code here, redeem it on the machine
fleet admin machines pair-code --org org_default --name dev-machine
fleet daemon pair --server ws://127.0.0.1:8787/ws/machine --code <code>

fleet network detect
fleet network wizard --mode tailscale --yes
fleet network wizard --mode cloudflare --hostname fleet.example.com

# preview / execute the authorized one-shot setup
fleet network connect --provider tailscale
fleet network connect --provider cloudflare --hostname fleet.example.com \
  --execute --access-email you@example.com
```

Role commands share `--config`, `--state-dir`, `--bin`, `--user`, and `--system`.
Without a scope flag the CLI uses system scope as root and user scope otherwise.
User scope installs a system unit at `/etc/systemd/system` that runs as the
invoking account; system scope uses `/etc/fleet` and `/var/lib/fleet`.
Without root the CLI skips privileged writes and prints the commands instead.
`--dry-run` writes and executes nothing, and `--yes` accepts defaults.
`start`/`stop`/`restart`/`status`/`logs` use systemd when the unit is installed
and `systemctl` is usable, otherwise the fallback supervisor. The supervisor
writes `<state>/run/supervisor.json`, logs to `<state>/logs/`, restarts crashes
with backoff, and rotates logs at 10 MiB. `status` exits 0 healthy, 1 stopped,
2 unhealthy/stale/conflict. Top-level `fleet status` summarizes both roles;
`fleet doctor` runs the same read-only config/permission/process checks and
exits 1 only on FAIL.

Global flags are `--admin-url`, `--admin-token`, `--cli-config` and `--json`;
the URL/token also come from `FLEET_ADMIN_URL` / `FLEET_ADMIN_TOKEN` or
`~/.fleet/cli.toml`. The network wizard only writes configuration
(admin `server.public_url`, daemon `connection.server_url`, CLI `public_url`,
and the Cloudflare tunnel YAML when the tunnel already exists). It never writes
a machine credential: enroll the daemon with `fleet daemon setup` (or
`fleet-daemon pair`) after the target URL is reachable. The wizard prints the
`tailscale serve` / `cloudflared tunnel` commands instead of executing them.

`network connect` upgrades this to the vendors' browser authorization
(`tailscale up`, `cloudflared tunnel login`). External and privileged commands
run only with `--execute`, and are confirmed one by one unless `--yes` is
passed. It can also create a Cloudflare Access app/policy
(`--access-email`, `--access-domain`, `--access-everyone`) using a scoped API
token stored in the OS keyring (0600 fallback file), and install cloudflared as
a system service on Linux. Use `--skip-access` / `--skip-service` to opt out.

`fleet admin sessions close` archives a session (status `terminated`, history
kept, hidden from the default session list); `fleet admin sessions delete --yes`
permanently removes the session and its turns, messages and permissions.
`fleet admin machines delete` revokes the machine and disconnects it;
`fleet admin machines revoked` and `fleet admin machines unrevoke <id>` list and
restore revocations.

## Database migrations

The admin and daemon have independent migration sets:

- `crates/admin-storage/migrations/` — machines, workspaces/session metadata,
  permissions, audit, organizations, users, machine ownership and pairing codes
  (`0005_pairing.sql`).
- `crates/daemon-storage/migrations/` — workspaces, directory index, sessions,
  turns, messages, tool calls and permissions.

Each set is append-only: SQLx stores a SHA-384 checksum in `_sqlx_migrations`,
so editing an already-applied file makes existing databases fail to start. Add
a new numbered migration instead of changing an old one (even comments).
Databases are created fresh under `~/.fleet`; this iteration has no migration
path from pre-rebrand databases. Domain models are shared through
`fleet-core-protocol`; storage code and schemas are not.

## Provider smoke checks

The default provider is `mock`, which needs no credentials and is used by the
E2E tests. Real providers:

- Claude Code CLI: `claude -p --input-format stream-json --output-format stream-json --verbose`
- Codex app server: `codex app-server --listen stdio://`
- OpenCode: the daemon starts `opencode serve --hostname 127.0.0.1 --port 0`
  and reads the printed URL/password; set `OPENCODE_SERVER_URL` (and optionally
  `OPENCODE_SERVER_PASSWORD`) to reuse an existing v2 server instead.

Adapter protocol fixtures live next to the adapters in
`crates/daemon-runtime/src/<provider>/fixtures/` and are refreshed whenever the
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
allow_writes` / `FLEET_GIT_WRITES` and the optional
`git.write_token` / `FLEET_GIT_WRITE_TOKEN` (browser equivalent: the
token dialog's Git write token field).

Rotate a machine key while its daemon is stopped:

```bash
fleet-daemon --config /path/to/config.toml rotate-key
fleet-daemon --config /path/to/config.toml serve
```

The stable machine id lives in `<key_file>.id`; the old key is kept in
`<key_file>.previous` only until fleet-admin accepts the rotation proof.
The per-machine token in `<key_file>.token` stays valid across rotation. If the
old key is lost, have an admin revoke and unrevoke the machine, then pair again
with a fresh code.

## Machine enrollment

`fleet daemon setup` is the supported path. It needs the admin REST API to be
reachable. Without an admin token it prompts for a pairing code (or an admin
token on a trusted host); scripts can pass `--code <code>` or supply
`--admin-token` / `FLEET_ADMIN_TOKEN`:

```bash
fleet --admin-url http://127.0.0.1:8787 --admin-token "$FLEET_ADMIN_TOKEN" daemon setup \
  --org org_default \
  --server ws://127.0.0.1:8787/ws/machine \
  --config ~/.fleet/daemon/config.toml \
  --name dev-machine
```

Pairing codes default to a 600-second TTL (`auth.pair_code_ttl_secs`, or
`FLEET_PAIR_CODE_TTL` on the admin) and 5 failed attempts
(`auth.pair_code_max_attempts`). `fleet-daemon pair --server ... --code ...`
writes only `<key_file>.token`; it does not edit the daemon TOML.
`fleet daemon setup --configure-only` writes config without pairing. Plaintext
`ws://` to a non-loopback host is refused unless `--allow-insecure` is passed.

## Data directories

- Daemon: `~/.fleet/daemon/fleet-daemon.db`, `~/.fleet/daemon/machine.key`,
  `~/.fleet/daemon/machine.key.id`, `~/.fleet/daemon/machine.key.token`
- Fleet Admin: `~/.fleet/admin/fleet-admin.db`

The daemon database is the authoritative transcript store: it persists turns,
messages and tool calls (`agent_turns`, `agent_messages`, `agent_tool_calls`)
in addition to session metadata. On restart, active sessions are restored as
read-only `interrupted` records with `metadata.restored=true`; their transcripts
remain available and pending permissions become cancelled. The admin stores
only machine/workspace/session metadata plus permission-routing records; it
does not mirror transcript rows. `GET /sessions/{id}` proxies the daemon, and
offline history is reported as unavailable instead of being served from a
partial cache.

Delete these files for a clean slate (machines must re-register). Separately,
deleting a machine through the API records a revocation in
`machine_revocations`; its daemon will not re-pair until an admin unrevokes it.

## Testing

- Rust unit tests live in each crate.
- Cross-crate integration tests live in `tests/`.
- Black-box E2E tests live in `crates/e2e-tests`. They start the real
  `fleet-admin`, `fleet-daemon` and `fleet` binaries on ephemeral loopback
  ports with a temporary `HOME`, database and Git repositories. They are
  `#[ignore]`d so `cargo test --workspace` stays fast and never spawns servers.
- The `mock` provider needs no network. The suite covers workspace
  tree/file/ETag reads, Git status/diff, streaming chat, permissions,
  interrupts, the durable outbox, archive/delete, revocation, role setup,
  the fallback supervisor lifecycle, Git writes with the dedicated token,
  multi-machine isolation/recovery and Ed25519 key rotation:

```bash
pnpm e2e         # smoke, setup, lifecycle and Git writes
pnpm e2e:ui      # Playwright browser E2E (install Chromium once)
pnpm e2e:stress  # three daemons, six workspaces, ~120 concurrent requests
```

- `pnpm e2e` builds the three binaries first. Artifacts are kept on failure;
  set `FLEET_E2E_KEEP=1` to keep them on success too.
- Run unit/integration tests with `pnpm test:rust`; the E2E commands above are
  separate because they spawn servers and browsers.

## Security notes for development

- The admin binary has no built-in default credential (the `pnpm dev` scripts
  pass `dev-token` explicitly). Configure `FLEET_ADMIN_TOKEN` (or
  `--admin-token`). Machines authenticate with per-machine tokens obtained via
  pairing; the admin token is not accepted on the machine WebSocket and there
  is no shared-token fallback. Production web builds embed no token and show
  the SPA auth gate instead.
- Sessions should pass `workspace_id`. A bare `cwd` is only accepted when it
  canonicalizes inside a workspace already registered on that machine.
- Machine identity is an Ed25519 key pair plus a per-machine token. The admin
  pins the key only during a successful pairing handshake; a key change requires
  a rotation proof signed by the previously pinned key. Losing both the token
  and the old key requires an admin revoke/unrevoke and a fresh pairing code.
- Deleting a machine records a permanent revocation and disconnects it; the
  daemon stops reconnecting on a revoked handshake. Use
  `fleet admin machines unrevoke <id>` (or `POST /api/v1/machines/{id}/unrevoke`) to
  allow pairing again.
- Every workspace path is resolved by `PathPolicy`; `..`, absolute paths and
  symlinks escaping the workspace root are denied.

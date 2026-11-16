# Security Model (MVP)

## Trust boundaries

- **Browser** — untrusted input. Only talks to the fleet-admin over HTTPS/WSS
  in production, HTTP/WS on loopback in development.
- **Agent Admin** — authenticates users, authorizes requests per machine, and
  never stores repository contents.
- **Machine daemon** — the only component that touches the local filesystem,
  Git repositories and agent processes. It enforces workspace path policy.
- **Coding agents** — may execute arbitrary commands by design. They are
  constrained by whatever the provider supports (permission approvals) and by
  the daemon's process management.

## User authentication and authorization

Every REST request and the browser event WebSocket must present a bearer token:

```
Authorization: Bearer <TOKEN>
```

- The legacy `FLEET_ADMIN_TOKEN` is a super-admin REST credential: it can
  see and manage every organization and is used for first-time bootstrap. It is
  never accepted on the machine WebSocket; machines authenticate with a
  per-machine token obtained by redeeming a short-lived pairing code. This keeps
  a leaked user credential from pairing a daemon.
- Additional users are created by an administrator (`POST /api/v1/users`) and
  receive an `flu_<64 hex>` token exactly once. Only its SHA-256 hash is stored;
  lookups use a unique index and comparisons do not leak timing.
- Each user belongs to exactly one organization and has one of three roles:
  - `admin`: manage users, organizations, machines and all writes;
  - `member`: use machines/workspaces/sessions and perform Git writes;
  - `viewer`: read-only; non-GET requests are rejected with 403.
- Organization scoping is derived from resource ownership:
  machine → organization, workspace/session/permission → machine. Cross-org
  lookups return 404. Machine lists, workspace lists, session lists and the
  audit log are filtered to the caller's organization.
- The fleet-admin can move a machine between organizations with
  `PUT /api/v1/machines/{id}/org`; organization admins can only assign machines
  they already own to their own organization.
- Token rotation (`POST /api/v1/users/{id}/rotate-token`) replaces the token
  hash immediately; the previous token stops working on the next request.
- `dev-token` is accepted only when explicitly configured; set
  `FLEET_ADMIN_TOKEN` in any shared environment.
- Production web builds embed no default token. `pnpm dev` / `pnpm dev:web`
  inject `VITE_DEFAULT_API_TOKEN` from `FLEET_ADMIN_TOKEN` (default
  `dev-token`) for local development only. If the SPA has no stored token or a
  request returns 401, it shows an auth gate and asks for a token rather than
  sending unauthenticated requests.
- The browser WebSocket accepts `?token=` because browsers cannot set headers on
  WebSocket upgrades. Use WSS so the token is not exposed in transit.

## Machine authentication

Each daemon owns an Ed25519 key pair stored at `~/.fleet/daemon/machine.key`
(0600, group/world-readable files are rejected) and, after enrollment, a
per-machine token stored at `<key_file>.token` (0600).

### Enrollment

1. An admin creates a single-use pairing code for an organization
   (`POST /api/v1/pairing/codes`, `fleet admin machines pair-code`, or
   `fleet daemon setup`, which performs both steps). Codes use 12 Crockford
   Base32 symbols, expire after
   `auth.pair_code_ttl_secs` (default 600) and allow
   `auth.pair_code_max_attempts` (default 5) failed attempts. Only a SHA-256
   hash of the code is stored.
2. `fleet daemon setup` (or `fleet-daemon pair`) opens `WS .../ws/machine`, sends
   `connection.hello` with the machine id, public key and pairing code, and
   signs the admin's 32-byte challenge.
3. The admin validates the code, verifies the signature with `verify_strict`
   Ed25519 semantics, pins the public key, consumes the code atomically and
   returns a per-machine token (`flm_<64 hex>`; only its hash is stored) in
   `connection.accepted`.
4. The daemon writes the token to `<key_file>.token` with mode 0600. Every later
   connection presents it as `Authorization: Bearer <machine-token>` plus a
   signed challenge. The pairing code is single-use and is not written to
   configuration.

There is no shared machine token, no trust-on-first-use path and no key-only
mode. `connection.token`, `FLEET_MACHINE_TOKEN` and `FLEET_TOKEN` are not read
anywhere. `fleet daemon setup` writes `connection.server_url`, `machine.id`
and the optional `machine.name` into the daemon TOML; it removes the legacy
`connection.token` / `connection.pairing_code` keys and stores the credential
only in the token file.

An unpaired machine is rejected with `machine_not_paired`; a pinned machine that
presents no token gets `machine_token_required`. Invalid, expired, already-used
or attempt-exhausted codes all return `pairing_code_invalid` so a caller cannot
distinguish the cases. A machine id whose organization no longer matches the
pairing code is rejected with `machine_org_mismatch`, and a new key without a
valid rotation proof with `machine_key_mismatch`.

### Machine key rotation

A changed key is accepted only when the daemon proves the rotation was
authorized by the previously pinned key:

1. `fleet daemon rotate-key` (daemon stopped) writes a fresh key to the key
   file and keeps the old key in `<key_file>.previous` (mode 0600).
2. The machine id is persisted in `<key_file>.id`, so rotation does not create a
   new machine identity.
3. On the next handshake the daemon presents the new key and, in
   `connection.auth`, a `KeyRotationProof` containing the new public key and a
   signature by the old key over
   `agent-fleet-key-rotation-v1\nmachine_id=…\nnew_public_key=…\nnonce=…`.
4. The fleet-admin verifies the challenge signature with the new key and the
   rotation statement with the pinned old key, then updates the pin. The nonce
   makes each proof single-use.
5. After the fleet-admin accepts the connection, the daemon deletes
   `<key_file>.previous`.

### Machine revocation

`DELETE /api/v1/machines/{id}` is a permanent revocation, not just a row
delete:

1. The machine id, timestamp, reason and owning organization are written to
   `machine_revocations`, and the same transaction clears
   `machines.token_hash` / `token_created_at`, so the credential is dead from
   the moment revocation commits.
2. The live connection is disconnected.
3. The machine and its dependent rows (workspaces, sessions, turns, messages,
   permissions) are removed from the active fleet; the revocation record is
   retained separately.

A daemon that reconnects with a revoked machine id is rejected during the
handshake; the key-pinning path re-checks the revocation immediately before
pinning so a revocation racing the handshake cannot resurrect the machine. The
client treats revocation as terminal and stops reconnecting rather than
entering its backoff loop. `GET /api/v1/machines/revoked` lists only the
caller's organization's revoked machines (the legacy super-admin sees all) and
`POST /api/v1/machines/{id}/unrevoke` removes a revocation and restores the
recorded organization scope and pinned public key. The CLI exposes these as
`fleet admin machines revoked` and `fleet admin machines unrevoke <id>`.

A revoked machine cannot pair again until an admin unrevokes it, even with a
fresh pairing code; the gateway rejects the machine id before validating the
code. Re-pairing an existing machine with a different key still requires the
rotation proof described above. Revocation destroys the stored token hash in the
same transaction that records the revocation, so a crash before the machine row
is deleted cannot leave a live credential. Unrevocation never restores a token:
the machine must pair again with a new code. If the token file alone was lost,
create a new code and pair again with the same pinned key; if the key itself was
lost, the pinned key must be reset by an administrator before a fresh key can
pair.

## Workspace isolation

- The browser never supplies an absolute filesystem path. It supplies a
  workspace id plus a workspace-relative path.
- `PathPolicy` canonicalizes the workspace root at registration and:
  - rejects absolute paths and any `..`/root/prefix component;
  - canonicalizes the target and verifies it is still inside the root, which
    defeats symlink escapes;
  - rejects non-regular files for content reads.
- File reads are capped (2 MiB default); larger content is truncated and marked.
- Binary files are detected and returned without content.
- `workspace.allowed_roots` optionally restricts where workspaces may be
  registered; registration and every later access canonicalize the path and
  reject the filesystem root. Without it, any directory the daemon user can
  read may be registered, so run the daemon as a dedicated user.
- Sessions prefer an explicit `workspace_id`. When only a bare `cwd` is sent,
  the daemon canonicalizes it and requires it to live inside a workspace
  already registered on that machine; arbitrary host paths are rejected with
  `path_denied`.
- `DELETE /api/v1/workspaces/{id}` returns HTTP 409 while any active session
  belongs to the workspace. Close or delete those sessions first so an agent
  cannot keep writing to an unregistered directory.

## Git operations

- Read commands: `status`, `diff`, `branches`, `log`, repository discovery.
  Write commands: `add`, `restore --staged`, `commit` and `switch` (Git 2.23+
  for `restore`/`switch`).
- Commands are executed with argument vectors (never a shell), an explicit
  working directory, a timeout and a bounded output buffer. User-supplied
  paths always follow `--` so they cannot be parsed as options.
- Staged paths are validated: absolute paths, `..`, `.git` and pathspec magic
  (`:...`) are rejected. Branch names are checked against Git's own ref rules.
  Commit messages are required and capped at 64 KiB.
- Untracked-file diffs re-check that each path resolves inside the workspace
  root before reading.
- Every Git command runs from the workspace root and is confined to it: status,
  diff, summary and log pass `--relative` and an explicit `-- .` pathspec, and
  repository status paths have the `--show-prefix` prefix stripped. A workspace
  rooted in a repository subdirectory therefore never reports or touches files
  above that directory.
- Revisions are resolved with `rev-parse --verify --end-of-options <rev>^{commit}`
  so option-like strings and ambiguous revisions cannot smuggle extra Git
  arguments.
- Write authorization is independent of the read API:
  - `git.allow_writes = false` (or `FLEET_GIT_WRITES=0`) disables all
    write endpoints (HTTP 403).
  - `git.write_token` (or `FLEET_GIT_WRITE_TOKEN`) adds a second
    credential: write requests must also send
    `X-Fleet-Git-Token`, verified in constant time (HTTP 401 otherwise).
  - Every attempt, including denials and Git failures, is appended to
    `audit_logs` and exposed via `GET /api/v1/audit`. Audit write failures are
    logged loudly but do not roll back an already-executed Git command.
- Not exposed: force-push, merge/rebase, reset --hard, tag/remote operations,
  history rewriting, or arbitrary Git subcommands.
- A checkout blocked by local modifications returns HTTP 409; an empty commit
  likewise returns 409 instead of a generic 500.

## Agent execution

- Only `fleet-daemon-runtime` spawns agent processes, through
  `fleet-daemon-process`, which places children in their own process
  group and kills the group on drop to avoid orphans.
- Providers declare capabilities honestly. The Claude CLI adapter in print
  mode cannot surface interactive permission prompts, so it declares
  `permission_approval = false` instead of faking it. Claude, Codex and
  Generic report `available: false` when their configured binary is missing,
  so the UI can explain why a session cannot start.
- A failed session start does not leave a ghost session: the daemon cleans up
  the runtime and its database rows, and emits `session.deleted`.
- Permission approvals are routed from the browser to the owning daemon and then
  to the provider's native mechanism. Pending requests can time out or be
  cancelled; the UI distinguishes approved/rejected/timed-out/cancelled/
  delivery-failed.
- On daemon restart, active sessions are restored as read-only `interrupted`
  records marked `metadata.restored=true`; their persisted turns, messages and tool calls
  remain viewable, pending permissions become `cancelled`, and send/resume are
  rejected. The admin independently marks sessions that a live daemon no
  longer reports as interrupted/orphaned.
- Agents run with the daemon user's privileges. The MVP does not sandbox the
  agent itself; run the daemon as a dedicated user on shared machines.

## Transport and request safety

- Production deployments must terminate TLS in front of the fleet-admin and
  use `https://` / `wss://` URLs in the daemon configuration.
- Every RPC has a request id, timeout and correlated response. Slow subscribers
  and oversized payloads are bounded (WebSocket frame/chunk limits, output
  caps). Malformed frames are logged and dropped, not fatal.
- The fleet-admin checks that the requesting user owns the machine that owns
  the workspace/session before forwarding a request.
- CORS is only enabled when a web origin is configured, and the allowed
  origins are limited to the local Vite dev server
  (`http://localhost:5173`, `http://127.0.0.1:5173`); methods include `PUT` and
  the custom `X-Fleet-Git-Token` header.

## Remote access

- See [`connectivity.md`](connectivity.md) for enrollment codes, LAN/WAN
  topologies, endpoint failover and transport fallbacks.
- The admin binds to loopback by default. Prefer Tailscale Serve (private
  tailnet with automatic TLS) or a Cloudflare Tunnel to the local port instead
  of exposing 8787 directly.
- Cloudflare tunnels are publicly reachable; put Cloudflare Access (or another
  identity-aware proxy) in front of the web UI. The daemon WebSocket still
  requires the machine token and its Ed25519 challenge, but that does not
  protect the browser login itself.
- The network wizard only writes configuration and prints privileged commands
  (`tailscale serve`, `cloudflared tunnel ...`); it never executes or escalates
  them. Config edits are backed up first.
- `fleet` stores its token only when explicitly asked (`fleet config init`),
  writes `~/.fleet/cli.toml` with mode 0600, and prefers `FLEET_ADMIN_TOKEN` for
  shared environments.
- `fleet network connect` uses the official browser authorization flows and only
  executes external commands with `--execute`. Privileged steps (`tailscale
  serve`, service installation) run through `sudo`; each step is confirmed
  unless `--yes` is passed. Cloudflare API tokens are stored in the OS keyring
  with a 0600 fallback, never in the service TOML files.
- On macOS, `--install` can run Homebrew, but Tailscale's VPN/network-extension
  approval and first-launch sign-in are GUI/system prompts. The CLI cannot grant
  them and must not try to bypass them.

## Data handling

- Agent Admin stores machine metadata, workspace paths, session metadata,
  permission records and audit entries. It never stores repository contents or
  a transcript copy. `GET /sessions/{id}` proxies the daemon; when the machine
  is offline the response is marked `metadata.history_unavailable=true` instead
  of serving a partial cached history.
- The daemon's SQLite database caches directory listings and session metadata
  and is the authoritative transcript store: it persists agent turns, messages
  and tool calls (`agent_turns`, `agent_messages`, `agent_tool_calls`). The
  filesystem and Git remain the sources of truth for repository contents.
- High-frequency `message.delta` events are not persisted as events; committed
  user/assistant message content, tool calls, session state and permission
  decisions are persisted.
- Closing a session archives it (`terminated`) and keeps its history. Deleting
  a session permanently removes its session row, turns, messages, tool calls and
  permissions from daemon storage, plus the admin metadata row.

## Known limitations (to address before multi-user/SaaS)

- Each user belongs to exactly one organization; cross-org guest access and
  per-workspace grants are not implemented.
- Tokens have no expiry; revoke access by rotating the token or deleting the user.
- Enrollment is implemented for same-host and same-LAN deployments only.
  Remote/offline/deferred enrollment, QR or OAuth device flow, and UI pairing
  are future work. `fleet daemon setup` needs a reachable admin HTTP API, a pairing code
  and write access to the daemon key directory.
- Upgrade throttling is an in-memory, per-source-IP limiter that only charges
  unauthenticated (pairing) upgrades; authenticated token reconnects are not
  throttled. It resets on admin restart and is not shared across admin
  instances. Per-code attempt limits are stored durably with the code. Behind a
  reverse proxy the peer address is the proxy.
- No general rate limiting or message-size enforcement at the HTTP layer yet.
- Agent processes are not sandboxed (no cgroups/seccomp/macOS sandbox profile).

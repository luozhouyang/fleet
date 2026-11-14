# Security Model (MVP)

## Trust boundaries

- **Browser** — untrusted input. Only talks to the agent admin over HTTPS/WSS
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

- The legacy `AGENT_FLEET_ADMIN_TOKEN` is a super-admin REST credential: it can
  see and manage every organization and is used for first-time bootstrap. It is
  not accepted on the machine WebSocket once `auth.machine_token` /
  `AGENT_FLEET_MACHINE_TOKEN` is configured (otherwise it is used as a
  backward-compatible fallback with a startup warning).
- Additional users are created by an administrator (`POST /api/v1/users`) and
  receive an `afu_<64 hex>` token exactly once. Only its SHA-256 hash is stored;
  lookups use a unique index and comparisons do not leak timing.
- Each user belongs to exactly one organization and has one of three roles:
  - `admin`: manage users, organizations, machines and all writes;
  - `member`: use machines/workspaces/sessions and perform Git writes;
  - `viewer`: read-only; non-GET requests are rejected with 403.
- Organization scoping is derived from resource ownership:
  machine → organization, workspace/session/permission → machine. Cross-org
  lookups return 404. Machine lists, workspace lists, session lists and the
  audit log are filtered to the caller's organization.
- The agent admin can move a machine between organizations with
  `PUT /api/v1/machines/{id}/org`; organization admins can only assign machines
  they already own to their own organization.
- Token rotation (`POST /api/v1/users/{id}/rotate-token`) replaces the token
  hash immediately; the previous token stops working on the next request.
- `dev-token` is accepted only when explicitly configured; set
  `AGENT_FLEET_ADMIN_TOKEN` in any shared environment.
- The browser WebSocket accepts `?token=` because browsers cannot set headers on
  WebSocket upgrades. Use WSS so the token is not exposed in transit.

## Machine authentication

Each daemon owns an Ed25519 key pair stored at `~/.agent-fleet/machine.key`
(0600, group/world-readable files are rejected).

1. The daemon opens `WSS .../ws/machine` with the machine token.
2. It sends `connection.hello` with its machine id, public key and version.
3. The agent admin pins the first public key seen for that machine id
   (trust on first use, gated by the user token).
4. The agent admin issues a 32-byte random challenge; the daemon signs it
   with `verify_strict` Ed25519 semantics using the presented key.
5. Only after successful verification is the connection registered and usable.

### Machine key rotation

A changed key is accepted only when the daemon proves the rotation was
authorized by the previously pinned key:

1. `agent-daemon rotate-key` (daemon stopped) writes a fresh key to the key
   file and keeps the old key in `<key_file>.previous` (mode 0600).
2. The machine id is persisted in `<key_file>.id`, so rotation does not create a
   new machine identity.
3. On the next handshake the daemon presents the new key and, in
   `connection.auth`, a `KeyRotationProof` containing the new public key and a
   signature by the old key over
   `agent-fleet-key-rotation-v1\nmachine_id=…\nnew_public_key=…\nnonce=…`.
4. The agent admin verifies the challenge signature with the new key and the
   rotation statement with the pinned old key, then updates the pin. The nonce
   makes each proof single-use.
5. After the agent admin accepts the connection, the daemon deletes
   `<key_file>.previous`.

Removing a machine clears the pinned key, allowing a re-paired machine to
register again.

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
  - `git.allow_writes = false` (or `AGENT_FLEET_GIT_WRITES=0`) disables all
    write endpoints (HTTP 403).
  - `git.write_token` (or `AGENT_FLEET_GIT_WRITE_TOKEN`) adds a second
    credential: write requests must also send
    `X-Agent-Fleet-Git-Token`, verified in constant time (HTTP 401 otherwise).
  - Every attempt, including denials and Git failures, is appended to
    `audit_logs` and exposed via `GET /api/v1/audit`. Audit write failures are
    logged loudly but do not roll back an already-executed Git command.
- Not exposed: force-push, merge/rebase, reset --hard, tag/remote operations,
  history rewriting, or arbitrary Git subcommands.
- A checkout blocked by local modifications returns HTTP 409; an empty commit
  likewise returns 409 instead of a generic 500.

## Agent execution

- Only `agent-fleet-agent-runtime` spawns agent processes, through
  `agent-fleet-process-manager`, which places children in their own process
  group and kills the group on drop to avoid orphans.
- Providers declare capabilities honestly. The Claude CLI adapter in print
  mode cannot surface interactive permission prompts, so it declares
  `permission_approval = false` instead of faking it.
- Permission approvals are routed from the browser to the owning daemon and then
  to the provider's native mechanism. Pending requests can time out or be
  cancelled; the UI distinguishes approved/rejected/timed-out/cancelled/
  delivery-failed.
- Agents run with the daemon user's privileges. The MVP does not sandbox the
  agent itself; run the daemon as a dedicated user on shared machines.

## Transport and request safety

- Production deployments must terminate TLS in front of the agent admin and
  use `https://` / `wss://` URLs in the daemon configuration.
- Every RPC has a request id, timeout and correlated response. Slow subscribers
  and oversized payloads are bounded (WebSocket frame/chunk limits, output
  caps). Malformed frames are logged and dropped, not fatal.
- The agent admin checks that the requesting user owns the machine that owns
  the workspace/session before forwarding a request.
- CORS is only enabled when a web origin is configured, and the allowed
  origins are limited to the local Vite dev server
  (`http://localhost:5173`, `http://127.0.0.1:5173`); methods include `PUT` and
  the custom `X-Agent-Fleet-Git-Token` header.

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
- `af` stores its token only when explicitly asked (`af config init`), writes
  `~/.agent-fleet/cli.toml` with mode 0600, and prefers
  `AGENT_FLEET_ADMIN_TOKEN` for shared environments.
- `af network connect` uses the official browser authorization flows and only
  executes external commands with `--execute`. Privileged steps (`tailscale
  serve`, service installation) run through `sudo`; each step is confirmed
  unless `--yes` is passed. Cloudflare API tokens are stored in the OS keyring
  with a 0600 fallback, never in the service TOML files.
- On macOS, `--install` can run Homebrew, but Tailscale's VPN/network-extension
  approval and first-launch sign-in are GUI/system prompts. The CLI cannot grant
  them and must not try to bypass them.

## Data handling

- Agent Admin stores machine metadata, workspace paths, session metadata,
  permission records and audit entries. It never stores repository contents.
- The daemon's SQLite database caches directory listings and session metadata;
  the filesystem and Git remain the sources of truth.
- High-frequency `message.delta` events are not persisted; only session state
  and permission decisions are.

## Known limitations (to address before multi-user/SaaS)

- Each user belongs to exactly one organization; cross-org guest access and
  per-workspace grants are not implemented.
- Tokens have no expiry; revoke access by rotating the token or deleting the user.
- Trust-on-first-use machine pairing has no interactive approval screen; the
  first connection is gated only by the shared daemon token.
- No rate limiting or message-size enforcement at the HTTP layer yet.
- Agent processes are not sandboxed (no cgroups/seccomp/macOS sandbox profile).

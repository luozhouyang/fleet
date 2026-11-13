# Security Model (MVP)

## Trust boundaries

- **Browser** — untrusted input. Only talks to the control plane over HTTPS/WSS
  in production, HTTP/WS on loopback in development.
- **Control plane** — authenticates users, authorizes requests per machine, and
  never stores repository contents.
- **Machine daemon** — the only component that touches the local filesystem,
  Git repositories and agent processes. It enforces workspace path policy.
- **Coding agents** — may execute arbitrary commands by design. They are
  constrained by whatever the provider supports (permission approvals) and by
  the daemon's process management.

## User authentication

The MVP is single-user. Every REST request and the browser event WebSocket must
present a bearer token:

```
Authorization: Bearer <ADMIN_TOKEN>
```

- Token comparison is constant-time.
- `dev-token` is accepted only when explicitly configured; set
  `AGENT_FLEET_ADMIN_TOKEN` in any shared environment.
- The browser WebSocket accepts `?token=` because browsers cannot set headers on
  WebSocket upgrades. Use WSS so the token is not exposed in transit.

## Machine authentication

Each daemon owns an Ed25519 key pair stored at `~/.agent-fleet/machine.key`
(0600, group/world-readable files are rejected).

1. The daemon opens `WSS .../ws/machine` with the user bearer token.
2. It sends `connection.hello` with its machine id, public key and version.
3. The control plane pins the first public key seen for that machine id
   (trust on first use, gated by the user token) and rejects later mismatches.
4. The control plane issues a 32-byte random challenge; the daemon signs it
   with `verify_strict` Ed25519 semantics.
5. Only after successful verification is the connection registered and usable.

Removing a machine in the UI clears the pinned key, allowing a re-paired machine
to register again. Key rotation is future work; the key file format is versioned
to make it possible.

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

## Git operations

- Only read-only Git commands are implemented (`status`, `diff`, `branches`,
  `log`, repository discovery).
- Commands are executed with argument vectors (never a shell), an explicit
  working directory, a timeout and a bounded output buffer.
- Untracked-file diffs re-check that each path resolves inside the workspace
  root before reading.
- Git writes (stage/commit/checkout) are out of scope for the MVP; when added
  they require separate authorization and audit records.

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

- Production deployments must terminate TLS in front of the control plane and
  use `https://` / `wss://` URLs in the daemon configuration.
- Every RPC has a request id, timeout and correlated response. Slow subscribers
  and oversized payloads are bounded (WebSocket frame/chunk limits, output
  caps). Malformed frames are logged and dropped, not fatal.
- The control plane checks that the requesting user owns the machine that owns
  the workspace/session before forwarding a request.

## Data handling

- Control plane stores machine metadata, workspace paths, session metadata,
  permission records and audit entries. It never stores repository contents.
- The daemon's SQLite database caches directory listings and session metadata;
  the filesystem and Git remain the sources of truth.
- High-frequency `message.delta` events are not persisted; only session state
  and permission decisions are.

## Known limitations (to address before multi-user/SaaS)

- Single shared admin token; no user accounts, organizations or per-machine ACLs.
- No token rotation or revocation list.
- Trust-on-first-use machine pairing has no interactive approval screen.
- No rate limiting or message-size enforcement at the HTTP layer yet.
- Agent processes are not sandboxed (no cgroups/seccomp/macOS sandbox profile).
- No Git write operations, therefore no write-audit surface.

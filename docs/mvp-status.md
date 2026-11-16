# MVP Status

Mapped to the phases in [`implementation.md`](implementation.md). All code is on
`main`; every claim below is covered by automated tests or the smoke script.

| Phase | Delivered | Evidence |
|---|---|---|
| 0 — Monorepo & tooling | Cargo workspace, pnpm workspace, root package.json scripts, rustfmt/clippy, CI | `cargo fmt --check`, `clippy -D warnings`, `.github/workflows/ci.yml` |
| 1 — Protocol | `crates/core-protocol`: agent/workspace/git/machine/connection types, RPC method contract, authz types, serde tests | 14 protocol tests |
| 2 — Daemon foundation | config loading (defaults < TOML < env), SQLite + migrations, process manager (pipe + PTY), Ed25519 identity | config 11, admin-storage 14, daemon-storage 16, daemon-process 16, auth 28 tests |
| 3 — Agent runtime | unified session state machine, event emitter/sequencer, permission hub, mock/Claude/Codex/OpenCode/generic adapters, persisted turns/messages/tool calls and restart restore, transactional outbox with at-least-once replay, honest provider availability | 61 runtime tests including one ignored (lib, mock, OpenCode mock server, Claude/Codex/OpenCode fixtures, restart restore, outbox cursors) |
| 4 — Workspace | registry, path policy, lazy listings + pagination, ignore rules, ETag reads, search, debounced watcher | 35 tests incl. traversal/symlink/allowlist tests |
| 5 — Git | discovery/worktrees, porcelain v2 parser, unified diff parser, untracked diffs, summaries, branches, log | 41 unit + 28 real-repo tests (incl. subdirectory confinement) |
| 6 — Reverse connection | Single-use pairing-code enrollment, per-machine tokens (`<key_file>.token`), Ed25519 challenge auth, rotation proof, heartbeat, reconnect, request routing, stream framing types | Loopback gateway tests (pairing, unpaired/token-required, reconnect, key/org mismatch, rotation proof, revocation) |
| 7 — Agent Admin | REST proxy (all documented routes), pairing-code API, machine registry with revocation/unrevocation, session archive vs permanent delete, daemon-owned transcript proxying, event pipeline, browser WS, SPA serving, bearer auth | 28 tests incl. loopback flow, ACL/roles, Git-write authorization, audit, scoped user auth, pairing codes and per-machine token auth |
| 8 — Web UI | machine dashboard, workspace explorer (lazy tree, Monaco viewer/diff), fleet sidebar with workspace-scoped sessions and inline session creation, Git panel, sessions with streaming, tool cards, permission approvals, reconnecting events, Git stage/commit/branch checkout and write-token settings | strict `tsc`, ESLint, Vite build, 6 Playwright UI tests |
| 9 — E2E | API smoke: machine online → workspace → tree/file/ETag → git → mock chat → permission → interrupt. Browser: dashboard → explorer/diff → file-change warning → session create/stream/permission/interrupt/close → Git stage/commit/checkout + audit | `scripts/test/e2e_smoke.sh`, `scripts/test/ui_e2e.sh` |
| 10 — Stability/security | timeouts, bounded output, malformed-frame tolerance, process-group cleanup, pairing-code enrollment, per-machine tokens, key rotation proof, permanent machine revocation, path policy, workspace allowlist, session cwd confinement, restart recovery (read-only interrupted sessions), multi-machine isolation and recovery, documented threat model | `docs/security.md`, pairing/reconnect/key-mismatch/revocation gateway tests, runtime restore tests, `scripts/test/multi_machine_stress.sh` |
| 11 — Git writes | stage/unstage/commit/checkout behind an independent write token, path/branch validation, conflict mapping, audit trail + listing API | 11 real-repo write/subdirectory tests + fleet-admin authorization tests + `scripts/test/git_write_e2e.sh` |
| 12 — Multi-user ACL & key rotation | organizations, per-user tokens with admin/member/viewer roles (management API), org-scoped machines/workspaces/sessions/audit, user token rotation, Ed25519 machine key rotation with old-key proof | storage/fleet-admin ACL tests, gateway rotation tests, stress script ACL + rotation sections |
| 13 — CLI & remote access | `fleet` management CLI over the admin REST API (machines incl. revoked/unrevoke, workspaces, sessions incl. close/delete, users, orgs, audit, permissions, config), Tailscale/Cloudflare detection, and an authorized `network connect` flow (browser login, tunnel/DNS/service, optional Cloudflare Access, keyring credentials, `--execute`/per-step confirmation) | 21 CLI unit tests + live CLI smoke against a running admin |
| 14 — Reproducible Linux verification | Docker test image with Rust, Node/pnpm and Playwright Chromium; quick/full/fresh verification modes | `docker/verify.Dockerfile`, `scripts/test/docker_verify.sh`, `pnpm docker:verify` |

**Totals:** 350+ Rust tests (1 ignored real-OpenCode test) + web
typecheck/lint/build + 6 Playwright UI tests + black-box API, Git-write and
multi-machine stress runs (3 daemons, 6 workspaces, ~100 concurrent requests).

## Deliberately deferred

Multi-instance agent admin (Redis/NATS), multi-tenant auth,
Tailscale/Cloudflare transports, Git merge/rebase/push operations, remote file editing,
worktree orchestration, agent sandboxing, an organization/user administration UI, and UI component unit tests.
Remote/offline/deferred pairing, QR or OAuth device flow and UI pairing are also
deferred; enrollment is currently same-host / same-LAN only. The per-IP upgrade
limiter is in-memory, resets on restart and is not shared across admin
instances.
OpenCode structured diffs and image attachments are not mapped yet.

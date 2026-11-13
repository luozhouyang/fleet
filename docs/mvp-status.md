# MVP Status

Mapped to the phases in [`implementation.md`](implementation.md). All code is on
`main`; every claim below is covered by automated tests or the smoke script.

| Phase | Delivered | Evidence |
|---|---|---|
| 0 — Monorepo & tooling | Cargo workspace, pnpm workspace, Makefile, rustfmt/clippy, CI | `cargo fmt --check`, `clippy -D warnings`, `.github/workflows/ci.yml` |
| 1 — Protocol | `crates/protocol`: agent/workspace/git/machine/connection types, RPC method contract, authz types, serde tests | 14 protocol tests |
| 2 — Daemon foundation | config loading (defaults < TOML < env), SQLite + migrations, process manager (pipe + PTY), Ed25519 identity | config 11, storage 11, process-manager 15, auth 28 tests |
| 3 — Agent runtime | unified session state machine, event emitter/sequencer, permission hub, mock/Claude/Codex/OpenCode/generic adapters | 24 lib + 16 mock/runtime + 5 OpenCode mock-server tests + Claude/Codex/OpenCode fixtures |
| 4 — Workspace | registry, path policy, lazy listings + pagination, ignore rules, ETag reads, search, debounced watcher | 33 tests incl. traversal/symlink tests |
| 5 — Git | discovery/worktrees, porcelain v2 parser, unified diff parser, untracked diffs, summaries, branches, log | 36 unit + 17 real-repo tests |
| 6 — Reverse connection | Ed25519 challenge auth, token gate, heartbeat, reconnect, request routing, stream framing types | 7 loopback tests (routing, bad token, reconnect, key mismatch, rotation proof) |
| 7 — Control plane | REST proxy (all documented routes), machine registry, event pipeline, browser WS, SPA serving, bearer auth | 22 tests incl. loopback flow, ACL/roles, Git-write authorization and audit |
| 8 — Web UI | machine dashboard, workspace explorer (lazy tree, Monaco viewer/diff), Git panel, sessions with streaming, tool cards, permission approvals, reconnecting events, Git stage/commit/branch checkout and write-token settings | strict `tsc`, ESLint, Vite build, 5 Playwright UI tests |
| 9 — E2E | API smoke: machine online → workspace → tree/file/ETag → git → mock chat → permission → interrupt. Browser: dashboard → explorer/diff → file-change warning → session create/stream/permission/interrupt/close → Git stage/commit/checkout + audit | `scripts/test/e2e_smoke.sh`, `scripts/test/ui_e2e.sh` |
| 10 — Stability/security | timeouts, bounded output, malformed-frame tolerance, process-group cleanup, TOFU key pinning, path policy, multi-machine isolation and recovery, documented threat model | `docs/security.md`, reconnect/key-mismatch tests, `scripts/test/multi_machine_stress.sh` |
| 11 — Git writes | stage/unstage/commit/checkout behind an independent write token, path/branch validation, conflict mapping, audit trail + listing API | 9 real-repo write tests + control-plane authorization tests + `scripts/test/git_write_e2e.sh` |
| 12 — Multi-user ACL & key rotation | organizations, per-user tokens with admin/member/viewer roles (management API), org-scoped machines/workspaces/sessions/audit, user token rotation, Ed25519 machine key rotation with old-key proof | storage/control-plane ACL tests, gateway rotation tests, stress script ACL + rotation sections |

**Totals:** 274 Rust tests (1 ignored real-OpenCode test) + web
typecheck/lint/build + 5 Playwright UI tests + black-box API, Git-write and
multi-machine stress runs (3 daemons, 6 workspaces, ~100 concurrent requests).

## Deliberately deferred

Multi-instance control plane (Redis/NATS), multi-tenant auth,
Tailscale/Cloudflare transports, Git merge/rebase/push operations, remote file editing,
worktree orchestration, agent sandboxing, an organization/user administration UI, and UI component unit tests.
OpenCode structured diffs and image attachments are not mapped yet.

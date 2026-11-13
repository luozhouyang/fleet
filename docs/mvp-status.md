# MVP Status

Mapped to the phases in [`implementation.md`](implementation.md). All code is on
`main`; every claim below is covered by automated tests or the smoke script.

| Phase | Delivered | Evidence |
|---|---|---|
| 0 — Monorepo & tooling | Cargo workspace, pnpm workspace, Makefile, rustfmt/clippy, CI | `cargo fmt --check`, `clippy -D warnings`, `.github/workflows/ci.yml` |
| 1 — Protocol | `crates/protocol`: agent/workspace/git/machine/connection types, RPC method contract, serde tests | 13 protocol tests |
| 2 — Daemon foundation | config loading (defaults < TOML < env), SQLite + migrations, process manager (pipe + PTY), Ed25519 identity | config 11, storage 11, process-manager 15, auth 25 tests |
| 3 — Agent runtime | unified session state machine, event emitter/sequencer, permission hub, mock/Claude/Codex/generic adapters | 20 lib + 16 mock/runtime tests + Claude/Codex fixtures |
| 4 — Workspace | registry, path policy, lazy listings + pagination, ignore rules, ETag reads, search, debounced watcher | 33 tests incl. traversal/symlink tests |
| 5 — Git | discovery/worktrees, porcelain v2 parser, unified diff parser, untracked diffs, summaries, branches, log | 36 unit + 17 real-repo tests |
| 6 — Reverse connection | Ed25519 challenge auth, token gate, heartbeat, reconnect, request routing, stream framing types | 5 loopback tests (routing, bad token, reconnect, key mismatch) |
| 7 — Control plane | REST proxy (all documented routes), machine registry, event pipeline, browser WS, SPA serving, bearer auth | 15 tests incl. loopback machine/workspace/session flow |
| 8 — Web UI | machine dashboard, workspace explorer (lazy tree, Monaco viewer/diff), Git panel, sessions with streaming, tool cards, permission approvals, reconnecting events | strict `tsc`, ESLint, Vite build, live browser smoke test |
| 9 — E2E | black-box script: machine online → workspace → tree/file/ETag → git → mock chat → permission → interrupt | `scripts/test/e2e_smoke.sh` |
| 10 — Stability/security | timeouts, bounded output, malformed-frame tolerance, process-group cleanup, TOFU key pinning, path policy, documented threat model | `docs/security.md`, reconnect/key-mismatch tests |

**Totals:** 238 Rust tests (236 + 2 new gateway tests) + web typecheck/lint/build
+ one end-to-end smoke run.

## Deliberately deferred

OpenCode adapter, multi-instance control plane (Redis/NATS), multi-tenant auth,
Tailscale/Cloudflare transports, Git write operations, remote file editing,
worktree orchestration, agent sandboxing, and UI unit/e2e test suites.

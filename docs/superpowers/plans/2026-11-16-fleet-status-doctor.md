# Fleet Status, Doctor, and Finalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the CLI surface with top-level `fleet status` and `fleet doctor`, verify the installer end to end, and update docs/examples.

**Architecture:** `fleet status` composes the per-role status reports from `process::status::collect`; `fleet doctor` runs read-only checks over both role configs, binaries, health endpoints, permissions, scope, and process conflicts. Both support `--json` and the documented exit codes.

**Tech Stack:** Rust, tokio, serde_json, existing Fleet config/auth/process modules.

---

## Decisions

- **D1** `fleet status` exit: 0 both healthy, 1 at least one stopped, 2 degraded/conflict.
- **D2** `fleet doctor` exit: 0 when no FAIL, 1 when any FAIL; WARN never fails.
- **D3** Doctor is read-only; every FAIL/WARN may carry an exact fix command.
- **D4** Both commands resolve role paths from `cli.toml` (`admin_config` / `daemon_config`) with the documented scope defaults.
- **D5** Doctor checks: binaries/versions, config parsing, DB separation, admin token prefix, daemon pairing, health, port conflict, systemd/supervisor conflict, token/config permissions, web assets, WebSocket URL, pairing TTL, `FLEET_*` overrides, scope/path match, and missing units.
- **D6** Setup/installer stay the only writers; status/doctor never mutate files or processes.

---

## File structure

- Create `apps/fleet-cli/src/fleet_status.rs` — both-role summary and aggregation.
- Create `apps/fleet-cli/src/doctor.rs` — read-only checks and rendering.
- Modify `apps/fleet-cli/src/process/status.rs` — expose `collect` without printing.
- Modify `apps/fleet-cli/src/cli.rs`, `main.rs` — wire top-level commands.
- Modify `scripts/test/lifecycle_e2e.sh` — assert top-level status and doctor against live roles.
- Modify `README.md`, `docs/development.md` — remove "Coming next" for these commands.

---

### Task 1: Both-role status

- [ ] Split `process::status::status` into `collect` → `(code, Value)` and a printer.
- [ ] Implement `fleet_status::run(admin, daemon, json)`:
  - aggregate codes (0/1/2);
  - human sections with scope, mode, health/version, config, database, URL, daemon machine/paired/server;
  - JSON `{ admin, daemon, ok }`.
- [ ] Unit-test aggregation precedence.
- [ ] Wire `Command::Status`; exit with the aggregate code.
- [ ] Commit `feat(cli): add top-level fleet status`.

### Task 2: Doctor

- [ ] Implement `Check { level, message, fix }` with PASS/WARN/FAIL.
- [ ] Run checks in order, tolerating missing files with WARN so a fresh host still gets useful output.
- [ ] Human output: `LABEL  message` plus `fix:` line; JSON `{ ok, checks[] }`.
- [ ] Wire `Command::Doctor`; exit 1 when any FAIL.
- [ ] Unit-test labels and fix hints.
- [ ] Commit `feat(cli): add read-only fleet doctor`.

### Task 3: E2E, installer, docs

- [ ] Extend `lifecycle_e2e.sh`:
  - configure and start an unpaired daemon under the supervisor;
  - assert top-level `fleet --json status` reports `ok` and both roles healthy;
  - assert `fleet doctor` exits 0 and prints PASS lines;
  - stop both roles and verify cleanup.
- [ ] Run `install_check.sh`, `setup_e2e.sh`, `lifecycle_e2e.sh`.
- [ ] Update README and `docs/development.md`; add `fleet status` / `fleet doctor` examples and exit codes.
- [ ] Commit `test(cli): cover status and doctor end to end`.

---

## Verification

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo fmt --all
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test --workspace
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo clippy --workspace --all-targets -- -D warnings
pnpm run test:web
./scripts/test/install_check.sh
./scripts/test/setup_e2e.sh
./scripts/test/lifecycle_e2e.sh
```

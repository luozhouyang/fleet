# Fleet Lifecycle and Fallback Supervision Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `fleet admin|daemon start|stop|restart|status|logs` with systemd delegation and a built-in detached supervisor fallback.

**Architecture:** `process::mode` selects systemd or supervisor per role. Systemd mode shells out to `systemctl`/`journalctl` without capturing sudo passwords. Supervisor mode spawns a detached hidden `fleet __supervise <role>` process that owns a state file, restarts the server with backoff, rotates logs, and forwards signals to the server process group. `run` stays the foreground `ExecStart` target.

**Tech Stack:** Rust, tokio (signals/process/time), libc (flock, setsid, PDEATHSIG, kill), serde_json, reqwest.

---

## Decisions

- **D1** `--mode auto` selects systemd only when the role's unit file exists at `/etc/systemd/system/<unit>` **and** `systemd_available()` is true; otherwise supervisor.
- **D2** Systemd commands run as the invoking user. On failure the CLI prints the exact `sudo systemctl ...` command and exits non-zero; it never runs sudo implicitly.
- **D3** Supervisor state lives in `<state-dir>/run/supervisor.json`; logs in `<state-dir>/logs/{admin|daemon}.log` and `supervisor.log`; lock in `run/start.lock`.
- **D4** Health URLs are derived from role config: admin `http://<host>:<port>/api/v1/health`; daemon `http://<host>:<port>/health`. Wildcard hosts probe `127.0.0.1`.
- **D5** `start` never starts a second instance: a reachable health endpoint short-circuits with exit 0.
- **D6** `stop` only signals processes whose `/proc/<pid>/cmdline` matches the expected role/supervisor command. A dead supervisor with a matching recorded child is cleaned up as stale; a mismatched live process is never signaled.
- **D7** Backoff: 1, 2, 4, 8, 16, 30 seconds (cap). Reset after the child stays alive for 60 seconds.
- **D8** Logs rotate at 10 MiB to `<file>.1` (copy + truncate) before start and every 60 seconds while supervising.
- **D9** Status exit codes: 0 running+healthy, 1 stopped, 2 unhealthy/stale, 3 internal error.
- **D10** `fleet admin run` / `fleet daemon run` refuse to exec when the health endpoint is reachable.
- **D11** macOS uses the same fallback supervisor without `PR_SET_PDEATHSIG`; Windows returns a clear unsupported error.

---

## File structure

- Create `apps/fleet-cli/src/process/health.rs` — role health URL, async probe, timeout.
- Create `apps/fleet-cli/src/process/supervisor.rs` — state, lock, start/stop/status/logs, hidden supervisor loop.
- Modify `apps/fleet-cli/src/process/systemd.rs` — unit-installed check, delegated start/stop/restart, journal logs.
- Modify `apps/fleet-cli/src/process/mod.rs` — mode selection and public lifecycle facade.
- Modify `apps/fleet-cli/src/cli.rs` — wire lifecycle commands, hidden `__supervise`.
- Create `scripts/test/lifecycle_e2e.sh` — real fallback supervisor start/status/logs/restart/stop.

---

### Task 1: Health probes and mode selection

**Files:** create `process/health.rs`; modify `process/mod.rs`.

- [ ] Write tests for:
  - `health_url(Role::Admin)` and `Role::Daemon` from temp configs.
  - wildcard host (`0.0.0.0`) maps to `127.0.0.1`.
  - `select_mode(unit_installed, systemd_available, requested)` returns systemd/supervisor and rejects `--mode systemd` when the unit is missing.
- [ ] Implement:

```rust
pub async fn probe_url(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    matches!(client.get(url).send().await, Ok(response) if response.status().is_success())
}
```

- [ ] `RolePaths` gains `run_dir()`, `logs_dir()`, `supervisor_state()`, `start_lock()`, `server_log()`, `supervisor_log()` returning paths under `state_dir`.
- [ ] Run `cargo test -p fleet-cli` and commit `feat(cli): add role health probes and mode selection`.

### Task 2: systemd delegation

**Files:** modify `process/systemd.rs`.

- [ ] Add `unit_installed(paths) -> bool`, `systemctl_action(paths, action)`, `journalctl(paths, lines, follow)`.
- [ ] Behavior:
  - `start`: run `systemctl start <unit>`; already-active returns success. On failure, print `sudo systemctl start <unit>` and return an error.
  - `stop`: run `systemctl stop`; inactive is success.
  - `restart`: run `systemctl restart`.
  - `status`: run `systemctl is-active --quiet` and capture state, `show -p UnitFileState`, `RestartUSec`.
  - `logs`: exec `journalctl -u <unit> -n <lines> [--follow]` with inherited stdio.
- [ ] Tests use a fake `systemctl` script in a temp `PATH` and assert the invoked argv plus failure hint. Never run the real systemctl.
- [ ] Commit `feat(cli): delegate lifecycle commands to systemd`.

### Task 3: Supervisor state, locking, and start/stop

**Files:** create `process/supervisor.rs`; modify `process/mod.rs`, `cli.rs`.

- [ ] State struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorState {
    /// Lowercase role name; avoids coupling the state file to `Role` serde.
    pub role: String,
    pub supervisor_pid: u32,
    /// `0` until the first server child is spawned.
    pub child_pid: u32,
    pub started_at: String,
    pub restarts: u32,
    pub last_exit: Option<String>,
    pub binary: PathBuf,
    pub config: PathBuf,
    pub server_log: PathBuf,
    pub supervisor_log: PathBuf,
}
```

- [ ] Helpers:
  - `read_state` returns `None` when absent or invalid.
  - `write_state` writes temp + rename with mode 0600.
  - `StartLock` acquires `flock(LOCK_EX)` on `run/start.lock`.
  - `pid_matches(pid, needle)` reads `/proc/<pid>/cmdline` on Linux; on macOS returns true only when `kill(pid, 0)` succeeds.
  - `signal(pid, sig)` and `signal_group(pid, sig)`.
  - `rotate_log(path, max_bytes)`.
- [ ] `start(paths, wait_limit)` (wire `ProcessArgs::timeout` into `wait_limit`):
  1. Acquire lock (creates `run/`).
  2. Probe health; if reachable print `already running` and return 0.
  3. If live supervisor state exists (even unhealthy), report `already running` and return 0.
  4. Refuse when the systemd unit is active.
  5. Validate config, admin token, DB separation, and binary.
  6. Create state/log dirs (0700), canonicalize paths, rotate logs.
  7. Spawn detached `fleet __supervise <role> --config ... --state-dir ... --bin ...` with `setsid`, stdin null, stdout/stderr appended to `supervisor.log`.
  8. Poll state + health up to `wait_limit`; on timeout signal the supervisor, remove state, and print the supervisor log tail.
- [ ] `stop(paths, wait_limit)`:
  1. Acquire lock.
  2. Read state; if absent, probe health: reachable → refuse (external), else `not running` exit 0.
  3. A dead/mismatched supervisor is stale: verify the child command line before signaling, then remove state.
  4. Verify supervisor pid command line; `SIGTERM`, wait `wait_limit`, `SIGKILL`.
  5. Verify the recorded child command line, then signal its process group if it survived.
  6. Remove state; print result.
- [ ] Hidden `__supervise` clap command accepts role/config/state-dir/bin and calls `supervisor::supervise` (wired together with the loop so each commit builds).
- [ ] Tests: state round-trip/0600, stale state cleanup, pid mismatch refusal, log rotation.
- [ ] Commit `feat(cli): add fallback supervisor state and lifecycle`.

### Task 4: Supervisor loop

**Files:** modify `process/supervisor.rs`.

- [ ] `supervise(role, config, state_dir, binary)`:
  - Write state before spawning.
  - Spawn `binary --config <config> serve` in its own process group with stdout/stderr appended to the server log.
  - On Linux set `PR_SET_PDEATHSIG(SIGTERM)`.
  - `tokio::select!` on child exit, SIGTERM, SIGINT, 60s rotation tick.
  - Unexpected exit: record `last_exit`, increment `restarts`, sleep backoff, respawn.
  - Reset backoff after 60s alive.
  - Stop: mark stopping, `SIGTERM` child group, wait 15s, `SIGKILL` if needed, remove state, exit 0.
- [ ] Test the backoff calculation and `last_exit` formatting as pure functions; do not spawn servers in unit tests.
- [ ] Commit `feat(cli): supervise and restart fleet servers`.

### Task 5: Status, logs, run preflight, E2E

**Files:** modify `process/mod.rs`, `process/supervisor.rs`, `process/systemd.rs`, `process/health.rs`, `cli.rs`; create `scripts/test/lifecycle_e2e.sh`; update `package.json`, docs.

- [ ] `status(paths, mode)` returns `(exit_code, Value)`:
  - mode: systemd/supervisor/external/stopped/conflict.
  - systemd: active state, restart policy, health.
  - supervisor: pids, uptime, restarts, last exit, health.
  - config/database/URL and DB-distinct check.
  - Daemon adds identity, paired state, server URL, health.
- [ ] `logs(paths, mode, lines, follow, supervisor)`:
  - systemd → journalctl.
  - supervisor → tail server/supervisor log; `--follow` polls.
  - external → print health URL and manual hint.
- [ ] `run_role` preflights health and refuses if reachable.
- [ ] Wire `--json` on status; print human table otherwise. Exit with the
  contract codes through `std::process::exit` after printing. Detect
  `conflict` when systemd is active and a live supervisor state exists.
- [ ] `scripts/test/lifecycle_e2e.sh`: hermetic temp HOME, admin setup without service, `start --mode supervisor`, `status`, `logs`, `restart`, `stop`, assert state/log files and process cleanup.
- [ ] Update README/docs; add `e2e:lifecycle`.
- [ ] Full verification: fmt, tests, clippy `-D warnings`, install check, setup E2E, lifecycle E2E.
- [ ] Commit `test(cli): cover supervised lifecycle end to end`.

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

## Deferred to Plan 4

- Top-level `fleet status` and `fleet doctor`.
- Installer/packaging finalization and top-level E2E orchestration.

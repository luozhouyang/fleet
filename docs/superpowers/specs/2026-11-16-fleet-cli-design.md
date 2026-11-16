# Fleet CLI, Process Supervision, and systemd Integration

- **Status:** Draft for review
- **Date:** 2026-11-16
- **Scope:** CLI rebrand, admin/daemon command namespaces, separated config and
  database paths, systemd-primary supervision, fallback supervisor, status and
  doctor commands, environment variable rebrand, token prefix rebrand
- **Replaces:** the `af` CLI shape, the shared `~/.agent-fleet` layout, the
  `agent-admin` binary, and the `AGENT_FLEET_*` environment variables
- **Backward compatibility:** none. The product has not launched. Old binaries,
  environment variables, paths, token prefixes, and command names are removed.
- **Related:** `docs/superpowers/specs/2026-11-16-pairing-design.md`

## 1. Summary

Rename the user-facing CLI to `fleet` and split its surface into explicit
namespaces:

- `fleet admin ...` — control plane setup, REST management, service lifecycle.
- `fleet daemon ...` — worker setup, pairing, local workspace operations,
  service lifecycle.
- `fleet config ...` — CLI client configuration.
- `fleet network ...` — cross-role remote access configuration.
- `fleet setup` — interactive role selection that only forwards to the role
  setup commands.
- `fleet status` — both-role summary.
- `fleet doctor` — diagnostics.

Admin and daemon are separate server binaries:

```text
fleet          user CLI, orchestrator, systemd frontend, fallback supervisor
fleet-admin    admin server binary (thin entrypoint over a library crate)
fleet-daemon   daemon server binary (thin entrypoint over a library crate)
```

The server logic lives in library crates. The `fleet` binary does not link the
admin or daemon server crates. It discovers and manages the two server
binaries as child processes or through systemd.

On Linux, **systemd is the primary supervisor**:

- `fleet-admin.service` and `fleet-daemon.service` run `fleet admin run` /
  `fleet daemon run` in the foreground.
- systemd provides boot autostart and crash restart.
- `fleet admin start|stop|restart|status|logs` delegate to
  `systemctl`/`journalctl`.
- A built-in supervisor is available as a fallback for hosts without systemd
  (development, macOS, explicit `--mode supervisor`).
- `fleet admin setup` and `fleet daemon setup` offer to install and start the
  unit by default. Privileged steps call `sudo` with inherited stdio, so sudo
  prompts on the terminal; the CLI never captures the password.
- User scope keeps config/state in `~/.fleet` and runs the unit as the invoking
  user. System scope uses `/etc/fleet` + `/var/lib/fleet` and dedicated
  `fleet-admin` / `fleet-daemon` users.

The interactive `fleet` process is never a daemon. In fallback mode it starts a
detached `fleet __supervise <role>` process and exits; that hidden supervisor
is the only long-lived `fleet` process.

Pairing codes and per-machine tokens remain the machine authentication model
from the pairing spec. This document does not change the pairing protocol.

## 2. Confirmed decisions

| # | Decision |
|---|---|
| 1 | `af` is renamed to `fleet`; `agent-admin` is renamed to `fleet-admin`; `fleet-daemon` keeps its name. No compatibility shims. |
| 2 | Three binaries remain: `fleet`, `fleet-admin`, `fleet-daemon`. Server logic lives in library crates and the server binaries are thin entrypoints. |
| 3 | All admin commands live under `fleet admin ...`; all daemon commands live under `fleet daemon ...`. |
| 4 | `fleet network` stays at the top level because it configures the admin public URL, the daemon server URL, and the CLI public URL together. |
| 5 | Configuration, database, runtime, and log paths use `~/.fleet/...`. |
| 6 | Admin and daemon use separate config files and databases: `~/.fleet/admin/config.toml` + `fleet-admin.db`, `~/.fleet/daemon/config.toml` + `fleet-daemon.db`. |
| 7 | Environment variables use the `FLEET_*` prefix. `AGENT_FLEET_*` is removed. |
| 8 | Token prefixes are rebranded: `fla_` for admin tokens, `flm_` for per-machine tokens, `flu_` for user tokens. |
| 9 | systemd is the primary Linux supervisor. CLI lifecycle commands delegate to `systemctl`/`journalctl`. |
| 10 | A built-in supervisor is retained as a fallback for hosts without systemd and for explicit `--mode supervisor`. |
| 11 | `fleet setup` is an interactive dispatcher only. |
| 12 | `fleet status` and `fleet doctor` are part of the current implementation scope. |
| 13 | `fleet admin setup` and `fleet daemon setup` offer to install and start the systemd service by default on Linux with systemd. `--install-service` / `--no-install-service` control it non-interactively. |
| 14 | Privileged installation uses `sudo` with inherited stdio. The CLI never captures, pipes, logs, or stores the sudo password. No TTY, cancelled sudo, or missing sudo leaves setup successful and prints the exact command. |
| 15 | User scope writes `~/.fleet` and installs a system unit that runs as the current user. System scope (`--system`) writes `/etc/fleet` + `/var/lib/fleet` and uses the dedicated `fleet-admin` / `fleet-daemon` users. |

## 3. Goals

- One user-facing command entry point: `fleet`.
- Obvious separation between admin and daemon operations.
- Separate config files and databases, enforced at setup and startup.
- systemd-managed boot autostart and crash restart on Linux.
- A reliable no-systemd fallback for development and other Unix hosts.
- CLI commands that work the same way in systemd and fallback modes, with the
  active mode always visible in `status`.
- A safe, explicit rebrand with no legacy aliases.
- Fast diagnostics through `fleet doctor`.

## 4. Non-goals

- Pairing protocol changes. Pairing codes and per-machine tokens are unchanged
  apart from the `flm_` prefix.
- Remote or offline pairing, QR codes, or OAuth device flow.
- A second production supervisor running alongside systemd.
- Windows service integration. The fallback supervisor targets Linux and macOS.
- Multiple admin instances per host.
- A web UI for process management.
- Migrating existing `~/.agent-fleet` data. New installs start from `~/.fleet`.

## 5. Binary and crate layout

### 5.1 Binaries

```text
apps/fleet-cli
  bin fleet

apps/fleet-admin
  lib fleet_admin
  bin fleet-admin

apps/fleet-daemon
  lib fleet_daemon
  bin fleet-daemon
```

- `fleet` is the only user-facing binary.
- `fleet-admin` and `fleet-daemon` are thin binaries: parse their small CLI,
  build the config, and call `fleet_admin::run(...)` /
  `fleet_daemon::run(...)`.
- `fleet` does not depend on `fleet-admin` or `fleet-daemon` crates. It
  starts them as child processes or asks systemd to do so.
- All three are installed together by the installer and version-checked by
  `fleet doctor`.

### 5.2 Why not one binary

Admin and daemon have different dependency and deployment boundaries:

- admin: axum, admin storage, web UI, REST API;
- daemon: agent runtime, workspace, Git, process spawning;
- admin runs on a server; daemons run on developer/workstation machines;
- they may run as different system users.

Merging them would put admin API/Web code on every daemon host and daemon
execution code on the admin host, increase binary size and build time, and blur
the least-privilege boundary. Keep separate server binaries; share logic
through crates.

### 5.3 Binary discovery

`fleet` resolves `fleet-admin` and `fleet-daemon` in this order:

1. `--bin <path>` on the role namespace;
2. `FLEET_ADMIN_BIN` / `FLEET_DAEMON_BIN`;
3. the directory containing the running `fleet` executable;
4. `PATH`.

If a binary is missing, the command fails with the resolved search paths and a
hint to install the server package. `fleet doctor` reports missing or
version-mismatched binaries.

## 6. Command model

### 6.1 Top level

```text
fleet [--json] [--cli-config <path>] [--admin-url <url>] [--admin-token <token>]
      <command>
```

Commands:

```text
fleet setup
fleet status
fleet doctor
fleet config  init|show|validate|path
fleet admin   <admin-command>
fleet daemon  <daemon-command>
fleet network detect|wizard|connect
```

- `--cli-config` points at `~/.fleet/cli.toml`.
- `--admin-url` and `--admin-token` override the CLI config for admin REST
  commands.
- `--json` applies to command output.

### 6.2 `fleet admin`

```text
fleet admin [--config <admin.toml>] [--state-dir <dir>] [--bin <path>]
            [--system|--user] <command>
```

Commands:

```text
setup
config show|path|validate
install-service
uninstall-service
start [--mode auto|systemd|supervisor]
stop [--mode auto|systemd|supervisor]
restart [--mode auto|systemd|supervisor]
status [--mode auto|systemd|supervisor] [--json]
logs [-f] [-n <lines>] [--mode auto|systemd|supervisor]
run
health

machines list|get|delete|revoked|unrevoke|reconnect|providers|move-org|pair-code
workspaces list|get|add|remove
sessions list|get|create|send|permissions|interrupt|resume|close|delete
users list|create|delete|rotate-token
orgs list|create
audit
permissions approve|reject
```

- `--config` defaults to `~/.fleet/admin/config.toml`.
- `--state-dir` defaults to `~/.fleet/admin`.
- `--bin` overrides the `fleet-admin` binary path.
- `--system` selects the system install scope (`/etc/fleet`,
  `/var/lib/fleet/admin`, unit user `fleet-admin`). `--user` forces the user
  scope (`~/.fleet`, unit user = current user). Default: root → system,
  non-root → user.
- `--mode auto` is the default. It selects systemd when a `fleet-admin.service`
  unit is installed and usable; otherwise it selects the fallback supervisor.
- `install-service` renders and enables the systemd unit (requires root).
- `uninstall-service` disables and removes the unit.
- `run` is the foreground exec used by the systemd unit.
- All REST client commands live under the `admin` namespace.

### 6.3 `fleet daemon`

```text
fleet daemon [--config <daemon.toml>] [--state-dir <dir>] [--bin <path>]
             [--system|--user] <command>
```

Commands:

```text
setup
config show|path|validate
install-service
uninstall-service
start [--mode auto|systemd|supervisor]
stop [--mode auto|systemd|supervisor]
restart [--mode auto|systemd|supervisor]
status [--mode auto|systemd|supervisor] [--json]
logs [-f] [-n <lines>] [--mode auto|systemd|supervisor]
run

pair --server <ws-url> --code <code>
providers
workspace add|list|remove
rotate-key
identity show
```

- `--config` defaults to `~/.fleet/daemon/config.toml`.
- `--state-dir` defaults to `~/.fleet/daemon`.
- `--bin` overrides the `fleet-daemon` binary path.
- `--system` selects the system install scope (`/etc/fleet`,
  `/var/lib/fleet/daemon`, unit user `fleet-daemon`). `--user` forces the user
  scope (`~/.fleet`, unit user = current user). Default: root → system,
  non-root → user.
- `pair` redeems a pairing code and writes `<key_file>.token`.
- `workspace add` is local daemon workspace registration. The admin API path
  remains `fleet admin workspaces add`.
- `identity show` prints machine id, key fingerprint, and token-file presence
  without printing the token.

### 6.4 `fleet config`

```text
fleet config init [--admin-url <url>] [--admin-token <token>] [--output table|json] [--force]
fleet config show [--reveal]
fleet config validate
fleet config path
```

`fleet config` manages only the CLI client configuration.

### 6.5 `fleet setup`

`fleet setup` is a dispatcher only:

```text
What do you want to set up?
  1) Admin / control plane
  2) Daemon / worker
  3) CLI only
```

It calls `fleet admin setup`, `fleet daemon setup`, or `fleet config init` with
the same arguments. It owns no setup logic and writes no files itself.

### 6.6 `fleet network`

`fleet network` stays top level:

```text
fleet network detect
fleet network wizard [--mode tailscale|cloudflare] [--hostname <host>] [--yes]
fleet network connect --provider tailscale|cloudflare [--execute] [--yes]
```

It updates:

- admin `[server] public_url`
- daemon `[connection] server_url`
- CLI `public_url`
- Cloudflare tunnel YAML when applicable

It uses `admin_config` and `daemon_config` from `~/.fleet/cli.toml`, falling
back to the defaults. It does not manage processes; it prints the relevant
`fleet admin restart` / `fleet daemon restart` commands.

## 7. Filesystem layout

### 7.1 User install

```text
~/.fleet/
  cli.toml
  cloudflared.yml
  secrets.toml

  admin/
    config.toml
    fleet-admin.db
    run/
      supervisor.json      # fallback supervisor only
      start.lock
    logs/
      admin.log            # fallback supervisor only
      supervisor.log       # fallback supervisor only

  daemon/
    config.toml
    fleet-daemon.db
    machine.key
    machine.key.id
    machine.key.token
    run/
      supervisor.json
      start.lock
    logs/
      daemon.log
      supervisor.log
```

- Directories: `0700`.
- `cli.toml`, config files, state files, logs, keys, tokens: `0600`.
- Cloudflare and keyring-fallback files move from `~/.agent-fleet/` to
  `~/.fleet/cloudflared.yml` and `~/.fleet/secrets.toml`.
- In systemd mode, server logs go to the journal and the local log files are
  not created.

### 7.2 System install

```text
/etc/fleet/
  admin.toml
  daemon.toml
  admin.env
  daemon.env

/var/lib/fleet/
  admin/
    fleet-admin.db
  daemon/
    fleet-daemon.db
    machine.key
    machine.key.id
    machine.key.token

/usr/local/share/fleet/web/
```

- systemd units run `fleet admin run --config /etc/fleet/admin.toml` and
  `fleet daemon run --config /etc/fleet/daemon.toml`.
- Logs go to the journal.
- The fallback supervisor is not used in system installs unless an operator
  explicitly requests `--mode supervisor` with a writable state directory.

### 7.3 Config files

`~/.fleet/admin/config.toml`:

```toml
[server]
host = "127.0.0.1"
port = 8787

[storage]
database = "/home/user/.fleet/admin/fleet-admin.db"

[auth]
admin_token = "fla_<64 hex>"
pair_code_ttl_secs = 600
pair_code_max_attempts = 5

[web]
static_dir = "/path/to/apps/web/dist"
```

`~/.fleet/daemon/config.toml`:

```toml
[machine]
name = "build-01"
id = "m_..."
key_file = "/home/user/.fleet/daemon/machine.key"

[storage]
database = "/home/user/.fleet/daemon/fleet-daemon.db"

[agent]
default_provider = "mock"

[connection]
server_url = "wss://fleet.example.com/ws/machine"
```

- The daemon config never contains a bearer token or pairing code.
- The per-machine token lives in `<key_file>.token`.
- Admin and daemon configs are never shared.

### 7.4 Database separation

- Admin database: `fleet-admin.db`.
- Daemon database: `fleet-daemon.db`.
- The CLI rejects any configuration where the resolved admin and daemon
  database paths are identical.
- The check runs in `fleet admin setup`, `fleet daemon setup`,
  `fleet admin start`, `fleet daemon start`, `fleet admin config validate`,
  `fleet daemon config validate`, and `fleet doctor`.
- No automatic database migration. Old `~/.agent-fleet` databases are ignored.

## 8. Environment variables

All project-specific environment variables use `FLEET_*`. There is no
`AGENT_FLEET_*` fallback.

| New | Replaces | Used by |
|---|---|---|
| `FLEET_CONFIG` | `AGENT_FLEET_CONFIG` | admin/daemon config path override |
| `FLEET_CLI_CONFIG` | `AGENT_FLEET_CLI_CONFIG` | CLI config path |
| `FLEET_ADMIN_URL` | `AGENT_FLEET_ADMIN_URL` | CLI admin base URL |
| `FLEET_ADMIN_TOKEN` | `AGENT_FLEET_ADMIN_TOKEN` | CLI/API admin token |
| `FLEET_SERVER_URL` | `AGENT_FLEET_SERVER_URL` | daemon WebSocket URL |
| `FLEET_PAIR_CODE` | `AGENT_FLEET_PAIR_CODE` | daemon pairing code |
| `FLEET_DB` | `AGENT_FLEET_DB` | SQLite path override |
| `FLEET_DATA_DIR` | `AGENT_FLEET_DATA_DIR` | data directory override |
| `FLEET_MACHINE_NAME` | `AGENT_FLEET_MACHINE_NAME` | daemon machine name |
| `FLEET_BIND` | `AGENT_FLEET_BIND` | admin/daemon bind address |
| `FLEET_WEB_DIR` | `AGENT_FLEET_WEB_DIR` | admin web assets |
| `FLEET_PAIR_CODE_TTL` | `AGENT_FLEET_PAIR_CODE_TTL` | admin pairing TTL |
| `FLEET_GIT_WRITES` | `AGENT_FLEET_GIT_WRITES` | Git write policy |
| `FLEET_GIT_WRITE_TOKEN` | `AGENT_FLEET_GIT_WRITE_TOKEN` | Git write token |
| `FLEET_DEV` | `AGENT_FLEET_DEV` | development token opt-in |
| `FLEET_SECRETS_FILE` | `AGENT_FLEET_SECRETS_FILE` | keyring fallback file |
| `FLEET_ADMIN_BIN` | new | admin binary override |
| `FLEET_DAEMON_BIN` | new | daemon binary override |

`RUST_LOG` remains the tracing filter and is not renamed.

The systemd env files use `FLEET_ADMIN_TOKEN` only. No machine token is stored
in environment files; per-machine tokens live next to the machine key.

`FLEET_DATA_DIR` is a base-directory override. To preserve separation it
derives role-specific subdirectories:

- admin: `<dir>/admin/fleet-admin.db`;
- daemon: `<dir>/daemon/fleet-daemon.db`, `<dir>/daemon/machine.key`, and the
  token/id files next to the key.

## 9. Token formats

| Token | Format | Storage |
|---|---|---|
| Admin token | `fla_<64 hex>` | admin config `[auth] admin_token`; CLI config `token` |
| Per-machine token | `flm_<64 hex>` | `<key_file>.token`, mode 0600 |
| User token | `flu_<64 hex>` | returned once by user creation/rotation; only its hash is stored |
| Pairing code | 12 Crockford Base32 symbols displayed `XXXX-XXXX-XXXX` | only its hash is stored |

- `fleet admin setup` generates `fla_` tokens.
- Pairing returns `flm_` tokens.
- `generate_user_token` returns `flu_` tokens.
- Pairing codes and hashing behavior are unchanged.
- Old prefixes (`af_admin_`, `afm_`, `afu_`) are not accepted or generated.

## 10. Process lifecycle and supervision

### 10.1 Modes

| Mode | How it starts | Restart owner | Boot autostart |
|---|---|---|---|
| systemd (primary) | `fleet-admin.service` / `fleet-daemon.service` runs `fleet ... run` | systemd | Yes, `systemctl enable` |
| supervisor (fallback) | `fleet admin start --mode supervisor` | `fleet __supervise` process | No |
| external | manual `fleet ... run` / container | runtime/operator | No |

`--mode auto` is the default:

1. if the role's systemd unit is installed and `systemctl` is usable, use
   systemd;
2. otherwise use the fallback supervisor.

`--mode systemd` requires the unit; it fails with installation instructions
when missing. `--mode supervisor` forces the fallback mode.

### 10.2 systemd delegation

In systemd mode the CLI is a frontend, not a supervisor:

| Command | Action |
|---|---|
| `start` | `systemctl start fleet-admin` / `fleet-daemon` |
| `stop` | `systemctl stop fleet-admin` / `fleet-daemon` |
| `restart` | `systemctl restart fleet-admin` / `fleet-daemon` |
| `status` | `systemctl status` + health probe + config/db summary |
| `logs` | `journalctl -u fleet-admin` / `fleet-daemon` with `-n`/`-f` |
| `run` | foreground exec of the server binary (used by ExecStart) |

- If `systemctl` returns a permission error, the CLI prints the exact `sudo
  systemctl ...` command and exits non-zero. It does not prompt for a password
  and does not run sudo itself.
- `start` is idempotent: an already-active unit reports `already running`.
- `stop` on an inactive unit reports `not running` and exits 0.
- `status` reports `mode: systemd`, unit state, health, config, database, and
  restart policy.

### 10.3 Fallback supervisor

The fallback supervisor is a hidden `fleet __supervise <role>` subcommand.

`fleet admin start --mode supervisor`:

1. Resolve config, state dir, binary, and log paths.
2. Validate the config, token rules, and DB separation.
3. Acquire `run/start.lock`.
4. Probe health:
   - healthy with a live supervisor state → already running; exit 0;
   - healthy without fleet state → already running externally; exit 0 with a
     mode hint;
   - not healthy → continue.
5. Rotate the server log if it exceeds the size limit.
6. Spawn a detached supervisor:
   ```text
   fleet __supervise admin --config <path> --state-dir <dir> --bin <path>
   ```
   The supervisor is detached with `setsid`, has `stdin = null`, and writes
   its own stdout/stderr to `logs/supervisor.log`.
7. Wait for `run/supervisor.json` and the child health probe.
8. Print pid, config, URL, log path, and mode.

The interactive `fleet` command exits after step 8.

Supervisor responsibilities:

- Write `run/supervisor.json` atomically:
  ```json
  {
    "role": "admin",
    "supervisor_pid": 4242,
    "child_pid": 4243,
    "started_at": "2026-11-16T12:00:00Z",
    "restarts": 0,
    "last_exit": null,
    "binary": "/home/user/.cargo/bin/fleet-admin",
    "config": "/home/user/.fleet/admin/config.toml",
    "server_log": "/home/user/.fleet/admin/logs/admin.log",
    "supervisor_log": "/home/user/.fleet/admin/logs/supervisor.log"
  }
  ```
- Spawn the server:
  - admin: `fleet-admin --config <config> serve`
  - daemon: `fleet-daemon --config <config> serve`
- Redirect the server's stdout/stderr to the server log.
- On Linux, set `PR_SET_PDEATHSIG(SIGTERM)` on the child so it terminates if
  the supervisor dies.
- Wait for the child.
- On unexpected exit:
  - record the exit status and timestamp;
  - increment `restarts`;
  - wait with exponential backoff before respawning;
  - update the state file.
- Backoff sequence: 1s, 2s, 4s, 8s, 16s, 30s (cap). Reset the sequence after
  the child has stayed alive for 60 seconds.
- On `SIGTERM` or `SIGINT`:
  - mark `stopping`;
  - forward `SIGTERM` to the child process group;
  - wait up to 15 seconds;
  - `SIGKILL` the child group if needed;
  - remove the state file and exit 0.
- `SIGHUP` is ignored. Use `fleet ... restart` for a config reload.
- Rotate logs every 60 seconds if either log exceeds 10 MiB, using
  copy-and-truncate so the server keeps writing to the same file descriptor.
  Keep one rotated file (`.1`).

The supervisor never writes tokens, pairing codes, or full Authorization
headers to logs.

### 10.4 Stop and restart

`stop` resolves the active mode:

- systemd: delegate to `systemctl stop`.
- supervisor: read `run/supervisor.json`; `SIGTERM` the supervisor; wait up to
  15s; `SIGKILL` the supervisor group if needed; remove the state file; verify
  the server is down and kill the child group if it survived.
- external: if the health endpoint is reachable, refuse to kill and print the
  manual/systemd hint; otherwise report `not running`.

`restart` is `stop` followed by `start` in the same mode. `--mode` can change
the mode; `auto` redetects.

### 10.5 Status

`fleet admin status` reports:

- mode: `systemd`, `supervisor`, `external`, or `stopped`;
- systemd unit state and restart policy, or supervisor/child pids, uptime,
  restart count, and last exit;
- config path, database path, URL, log source;
- health result and version;
- whether the database is distinct from the other role's database.

`fleet daemon status` additionally reports:

- machine id and name;
- paired/unpaired and the token-file path;
- server URL;
- health and active session count.

`status` exits 0 when running and healthy, 1 when stopped, 2 when running but
unhealthy or stale, and 3 on an internal error.

### 10.6 Logs

- systemd mode: `journalctl -u fleet-admin` / `fleet-daemon`, with `-n` and
  `-f`.
- supervisor mode: `logs/admin.log` or `logs/daemon.log`; `--supervisor`
  selects `logs/supervisor.log`.
- external mode: report that no fleet-managed log exists and print the health
  endpoint.

### 10.7 Foreground run

`fleet admin run` / `fleet daemon run`:

- Validate config and DB separation.
- Preflight the health endpoint. If already reachable, exit with an error:
  ```text
  admin is already running; stop it or use `fleet admin restart`
  ```
- Resolve the binary and exec it with `--config <path> serve`.
- On Unix use `exec`; on Windows spawn and wait, propagating the exit code.
- No PID file, no state file, no log redirection. systemd/containers capture
  stdout/stderr.

### 10.8 Conflict rules

- `start` never launches a second instance when the health endpoint is already
  reachable.
- `stop` and `restart` never kill a process that fleet did not start.
- `run` refuses to start when the health endpoint is reachable.
- systemd and fallback supervisor modes are mutually exclusive per role.
  `status` makes the active mode explicit.
- `fleet doctor` reports a conflict when a systemd unit is active and a
  supervisor state file also exists.

### 10.9 State, locking, and signals

- `run/start.lock` is an exclusive `flock` held during fallback start/stop.
- `run/supervisor.json` is written atomically (temp + rename) and is `0600`.
- The supervisor and child are in a dedicated process group created with
  `setsid`.
- Stop signals the supervisor; the supervisor signals the child group.
- On Linux, `/proc/<pid>/cmdline` is checked before signaling to avoid PID
  reuse.

### 10.10 Platform notes

- Linux: systemd primary; fallback supervisor supported.
- macOS: launchd is the system supervisor equivalent; the fallback supervisor
  is supported. `PR_SET_PDEATHSIG` is Linux-only, so a supervisor crash may
  leave the child running; `status` reports a stale state and `stop` can clean
  it up via the recorded child pid.
- Windows: out of scope for V1. `start --mode supervisor` returns a clear "not
  supported" error; `run` remains available for service wrappers.

## 11. systemd integration

### 11.1 Units

```text
packaging/systemd/fleet-admin.service.in
packaging/systemd/fleet-daemon.service.in
```

Admin unit:

```ini
[Unit]
Description=fleet admin (REST API + web UI)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=fleet-admin
Group=fleet-admin
Environment=HOME=/var/lib/fleet/admin
EnvironmentFile=-/etc/fleet/admin.env
ExecStart=/usr/local/bin/fleet admin run --config /etc/fleet/admin.toml
Restart=always
RestartSec=2
StartLimitIntervalSec=60
StartLimitBurst=10
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=/var/lib/fleet/admin
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

Daemon unit:

```ini
[Unit]
Description=fleet machine daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=fleet-daemon
Group=fleet-daemon
Environment=HOME=/var/lib/fleet/daemon
EnvironmentFile=-/etc/fleet/daemon.env
ExecStart=/usr/local/bin/fleet daemon run --config /etc/fleet/daemon.toml
Restart=always
RestartSec=2
StartLimitIntervalSec=60
StartLimitBurst=10
NoNewPrivileges=true
PrivateTmp=true
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

- `Restart=always` provides crash restart.
- `WantedBy=multi-user.target` plus `systemctl enable` provides boot
  autostart.
- `fleet admin run` / `fleet daemon run` exec the server binary, so systemd
  tracks a single process per service.
- `StartLimitIntervalSec`/`StartLimitBurst` prevent an unbounded crash loop.
  `fleet status` shows the failed unit and `fleet doctor` points at the logs.

The snippets above are the **system scope** units. User scope renders the same
units with:

- `User=<current user>` and `Group=<primary group>`;
- `Environment=HOME=<home>`;
- `ExecStart=/path/to/fleet admin run --config %h/.fleet/admin/config.toml`
  (expanded to the absolute home path at render time);
- no `/etc/fleet/*.env` file; the admin token lives in the `0600` config file
  under `~/.fleet`.

### 11.2 Service installation through the CLI

`fleet admin install-service` / `fleet daemon install-service`:

- Render the unit from the embedded template.
- Resolve `ExecStart` to the current `fleet` executable and the role config.
- Resolve the install scope:
  - **user scope** (default when not root): config and state stay under
    `~/.fleet/...`; the system unit runs as the current user and group with
    `Environment=HOME=<home>`;
  - **system scope** (`--system`, default when root): config lives in
    `/etc/fleet/...`, state in `/var/lib/fleet/...`, and the unit runs as the
    dedicated `fleet-admin` / `fleet-daemon` user.
- Write `/etc/systemd/system/fleet-admin.service` or
  `/etc/systemd/system/fleet-daemon.service`.
- For system scope, write `/etc/fleet/admin.env` / `daemon.env` when missing.
- Run `systemctl daemon-reload`.
- Run `systemctl enable --now <unit>`.
- Require root for the privileged steps. If the CLI is not root, it invokes
  `sudo` with inherited stdio so sudo prompts on the controlling terminal.
  The CLI never captures or pipes the password.
- Be idempotent: if the unit exists, re-render it; if it is active, reload and
  `enable --now`/`restart` as appropriate; never create duplicate units.

`fleet admin uninstall-service` / `fleet daemon uninstall-service`:

- Run `systemctl disable --now <unit>`.
- Remove the unit file.
- Run `systemctl daemon-reload`.
- Leave config, databases, keys, and tokens untouched.

#### Installation from setup

`fleet admin setup` and `fleet daemon setup` call the same `install-service`
logic when the user accepts the prompt or passes `--install-service`:

- Admin order:
  1. write admin config and CLI config;
  2. install and start the unit;
  3. wait for admin health;
  4. print pairing instructions.
- Daemon order:
  1. write daemon config;
  2. complete pairing and save `<key_file>.token`;
  3. install and start the unit;
  4. wait for daemon health.
- `--configure-only` with no existing token installs the unit but does not
  start it; setup prints the `fleet daemon pair` command that must run first.

#### Sudo behavior

- Interactive setup shows each privileged command before running it.
- `sudo` prompts on the terminal; the CLI does not use `sudo -S`, does not
  read the password with `rpassword`, and does not pass it on the command line.
- On non-TTY input, cancelled sudo, missing sudo, or missing systemctl:
  - do not fail setup;
  - keep the written configuration;
  - print the exact command to run later, with absolute paths;
  - suggest `fleet admin start --mode supervisor` as a temporary fallback.
- With `--yes`, automatic installation only proceeds when already root or when
  passwordless sudo is available (`sudo -n true`). Otherwise it prints the
  commands and exits 0.

#### Scope selection

- `--system`: force system scope. Requires root or sudo; uses dedicated service
  users and `/etc/fleet` + `/var/lib/fleet`.
- `--user`: force user scope. Uses `~/.fleet`; the unit runs as the current
  user and still starts at boot.
- Default: root → system scope; non-root → user scope.
- The scope is printed in the setup plan and in `fleet admin status` /
  `fleet daemon status` (`scope: user` or `scope: system`).

User scope intentionally still installs a **system** unit (not
`systemctl --user`) because the system unit starts at boot without
`loginctl enable-linger`. It runs as the invoking user and reads only that
user's `~/.fleet` files.

### 11.3 Installer

`scripts/install/install.sh` must:

1. Install `fleet`, `fleet-admin`, and `fleet-daemon`.
2. Remove or replace `af` and `agent-admin` installations.
3. Create service users `fleet-admin` and `fleet-daemon`.
4. Write `/etc/fleet/admin.toml`, `/etc/fleet/daemon.toml`,
   `/etc/fleet/admin.env`, and `/etc/fleet/daemon.env`.
5. Generate `FLEET_ADMIN_TOKEN` (`fla_...`) if absent.
6. Create `/var/lib/fleet/admin` and `/var/lib/fleet/daemon` with the correct
   ownership and modes.
7. Install and enable the units:
   ```bash
   systemctl daemon-reload
   systemctl enable --now fleet-admin fleet-daemon
   ```
8. Wait for admin health.
9. Pair the local daemon with `fleet daemon setup` using the admin token, or
   print the `fleet admin machines pair-code` + `fleet daemon pair` commands
   when the admin is not reachable yet.
10. Preserve ownership/modes after pairing (`machine.key`, `.id`, `.token`,
    config backups).

The installer runs as root and uses the same embedded unit templates and
system-scope rendering as `fleet admin install-service` /
`fleet daemon install-service`. It must not duplicate unit content.

### 11.4 systemd mode detection

- `fleet ... status` checks `systemctl is-active <unit>` and the unit file.
- If the unit is active and no fallback supervisor state exists, mode is
  `systemd`.
- If both are active, mode is `conflict`; `fleet doctor` reports it as a
  failure and explains how to stop the duplicate.
- `fleet ... start --mode auto` refuses to start a second instance when the
  health endpoint is already reachable.

## 12. Setup flows

### 12.1 `fleet admin setup`

Interactive prompts:

```text
Admin setup
  Config    ~/.fleet/admin/config.toml
  Database  ~/.fleet/admin/fleet-admin.db
  Bind      [127.0.0.1:8787]:
  Web UI    [apps/web/dist]:
  Admin token: (g)enerate / (r)euse
  Write ~/.fleet/cli.toml? [Y/n]: y
  Install scope: (u)ser home / (s)ystem-wide [user]:
  Install and start systemd service? [Y/n]: y
```

Behavior:

- Generate `fla_<64 hex>` when the token is missing.
- Reuse an existing token by default; rotate only with
  `--rotate-admin-token`.
- Write `[server] host/port`, `[storage] database`, `[auth] admin_token`, and
  optional `[web] static_dir`.
- Validate the rendered TOML with `AdminConfig::from_str`.
- Write `~/.fleet/cli.toml` with `admin_url`, `token`, `admin_config`, and
  `daemon_config` unless `--no-cli-config` is passed.
- On Linux with systemd, prompt to install and start the service, default
  `Yes`. `--install-service` / `--no-install-service` control it
  non-interactively.
- Scope defaults: root → `system`, non-root → `user`. `--system` / `--user`
  override it.
- User scope writes `~/.fleet` and installs a system unit that runs as the
  current user. System scope writes `/etc/fleet` + `/var/lib/fleet` and uses
  `fleet-admin`.
- Privileged steps run through `sudo` with inherited stdio. The CLI never
  captures the password. Cancelled sudo, no TTY, or missing sudo keeps the
  written config and prints the exact command.
- After a successful install, wait for admin health before printing the next
  steps.
- If service installation is skipped, print:
  ```text
  fleet admin start            # auto: systemd if installed, else supervisor
  # or force the fallback:
  fleet admin start --mode supervisor
  ```
- Print next steps:
  ```text
  fleet admin start
  fleet daemon setup
  fleet admin machines pair-code --org org_default --name <machine>
  ```
- Non-interactive flags:
  `--config`, `--bind`, `--database`, `--web-dir`, `--no-web`,
  `--rotate-admin-token`, `--no-cli-config`, `--install-service`,
  `--no-install-service`, `--system`, `--user`, `--start`, `--yes`,
  `--dry-run`.

### 12.2 `fleet daemon setup`

Interactive pairing choice:

```text
Pair how?
  1) Redeem a pairing code (recommended for worker machines)
  2) Use an admin token (same host / trusted LAN)
  3) Configure only (already paired)
```

Modes:

- **Pairing code**: hidden prompt or `--code`; normalize and validate locally;
  call `pair_machine` directly; no admin token required.
- **Admin token**: read from `~/.fleet/cli.toml`, `--admin-token`, or
  `FLEET_ADMIN_TOKEN`; verify admin health; create a pairing code via
  `POST /api/v1/pairing/codes`; pair.
- **Configure only**: write config without network calls.

Writes:

- `~/.fleet/daemon/config.toml`
- `[machine] name/id/key_file`
- `[storage] database = ~/.fleet/daemon/fleet-daemon.db`
- `[connection] server_url`
- Removes legacy `connection.token` and `connection.pairing_code` keys.
- Saves `<key_file>.token` during pairing.
- Optionally registers a workspace by invoking
  `fleet-daemon --config <path> register-workspace <root>`.

Service installation:

- After pairing succeeds, prompt:
  ```text
  Install and start systemd service? [Y/n]: y
  ```
- Install the unit only after `<key_file>.token` exists; the daemon must be
  paired before systemd starts it.
- With `--configure-only` and no existing token, install the unit but leave it
  stopped and print the `fleet daemon pair` command.
- Scope defaults: root → `system`, non-root → `user`. User scope writes
  `~/.fleet` and runs the unit as the current user; system scope writes
  `/etc/fleet` + `/var/lib/fleet` and runs as `fleet-daemon`.
- Privileged steps use `sudo` with inherited stdio; no password capture. On
  failure, keep the config/token and print the exact command.
- If installation is skipped, print:
  ```text
  fleet daemon start --mode supervisor
  ```

Flags:
`--config`, `--server`, `--code`, `--org`, `--name`, `--key-file`,
`--database`, `--workspace`, `--configure-only`, `--allow-insecure`,
`--install-service`, `--no-install-service`, `--system`, `--user`,
`--start`, `--yes`, `--dry-run`.

### 12.3 `fleet config init`

Writes `~/.fleet/cli.toml`:

```toml
admin_url = "http://127.0.0.1:8787"
token = "fla_<64 hex>"
output = "table"
public_url = "https://fleet.example.com"
admin_config = "/home/user/.fleet/admin/config.toml"
daemon_config = "/home/user/.fleet/daemon/config.toml"
```

Existing CLI config is backed up before overwrite.

### 12.4 `fleet network`

Uses the admin/daemon config paths from `~/.fleet/cli.toml` and updates
`[server] public_url`, `[connection] server_url`, and CLI `public_url` in one
pass. It prints the restart commands for the active systemd or supervisor
mode.

## 13. `fleet status`

`fleet status` summarizes both roles:

```text
$ fleet status

admin
  scope      system (User=fleet-admin)
  mode       systemd (fleet-admin.service active)
  health     ok (version 0.1.0, 2 machines online)
  config     /etc/fleet/admin.toml
  database   /var/lib/fleet/admin/fleet-admin.db
  url        http://127.0.0.1:8787

daemon
  scope      system (User=fleet-daemon)
  mode       systemd (fleet-daemon.service active)
  health     ok (active sessions: 0)
  machine    m_... (build-01)
  paired     yes
  config     /etc/fleet/daemon.toml
  database   /var/lib/fleet/daemon/fleet-daemon.db
  server     wss://fleet.example.com/ws/machine
```

- Reads systemd unit state and unit `User`/`HOME` when available.
- Reports `scope: user` (unit runs as current user) or
  `scope: system` (dedicated service user).
- Reads fallback supervisor state when present.
- Falls back to health probing for external instances.
- `--json` returns a structured object for scripts.
- Exit 0 when both roles are healthy, 1 when one is stopped, 2 when degraded.

## 14. `fleet doctor`

`fleet doctor` runs read-only checks and prints `PASS`, `WARN`, or `FAIL` with a
fix hint:

1. `fleet`, `fleet-admin`, and `fleet-daemon` are present and versions match.
2. Admin and daemon config files parse with `AdminConfig::from_str` /
   `DaemonConfig::from_str`.
3. Resolved admin and daemon database paths differ.
4. Admin token is present and starts with `fla_`.
5. Daemon `<key_file>.token` is present for a paired machine.
6. Admin and daemon health endpoints respond.
7. Ports from the two configs do not conflict.
8. systemd unit state and fallback supervisor state do not conflict.
9. Config, state, log, key, and token permissions are safe.
10. Admin `[web] static_dir` exists when configured.
11. daemon `connection.server_url` is a valid `ws://`/`wss://` URL and, when
    reachable, the host responds.
12. `FLEET_*` environment variables that override config are reported.
13. Pairing code TTL and attempt limits are sane.
14. No duplicate admin/daemon processes are listening.
15. The systemd unit `User=` and `HOME=` can read the configured config and
    database paths:
    - user scope: `User=<current user>`, `HOME=<home>`;
    - system scope: `User=fleet-admin` / `fleet-daemon`, state under
      `/var/lib/fleet`.
16. The install scope matches the config paths: user scope does not mix
    `~/.fleet` with a `fleet-admin` service user, and system scope does not mix
    `/etc/fleet` with a random user.
17. When a systemd install is expected but the unit is missing, report a
    `WARN` with the exact `sudo fleet ... install-service` command.

Flags:

- `--json` for machine-readable output.
- `--fix` is not part of V1; `doctor` is read-only and prints the exact command
  for each fix.
- Exit 0 when no `FAIL`; exit 1 when at least one `FAIL`; `WARN` does not fail
  the command.

## 15. CLI UX examples

### 15.1 systemd first run with setup auto-install

```text
$ fleet admin setup
Admin setup
  Scope     user (unit runs as user)
  Config    ~/.fleet/admin/config.toml
  Database  ~/.fleet/admin/fleet-admin.db
  Bind      [127.0.0.1:8787]:
  Web UI    [apps/web/dist]:
  Admin token: (g)enerate / (r)euse
  Write ~/.fleet/cli.toml? [Y/n]: y
  Install and start systemd service? [Y/n]: y

=> sudo install -m 0644 /tmp/fleet-admin.service /etc/systemd/system/fleet-admin.service
[sudo] password for user: ****
=> sudo systemctl daemon-reload
=> sudo systemctl enable --now fleet-admin
waiting for http://127.0.0.1:8787/api/v1/health...
admin installed and running (systemd, scope=user)

$ fleet admin status
admin: running (systemd)
  scope     user (User=user, HOME=/home/user)
  unit      fleet-admin.service active
  health    ok (version 0.1.0, 0 machines online)
  config    ~/.fleet/admin/config.toml
  database  ~/.fleet/admin/fleet-admin.db
  url       http://127.0.0.1:8787
```

If the user cancels sudo or no TTY is available:

```text
skipped systemd installation; configuration was written
run later:
  sudo /usr/local/bin/fleet admin install-service \
    --config /home/user/.fleet/admin/config.toml
temporary fallback:
  fleet admin start --mode supervisor
```

### 15.2 Fallback supervisor

```text
$ fleet admin setup --no-install-service
... writes ~/.fleet/admin/config.toml ...

$ fleet admin start --mode supervisor
admin started
  mode      supervisor
  pid       4242 (supervisor), 4243 (server)
  config    ~/.fleet/admin/config.toml
  database  ~/.fleet/admin/fleet-admin.db
  url       http://127.0.0.1:8787
  log       ~/.fleet/admin/logs/admin.log

$ kill -9 4243
$ fleet admin status
admin: running (supervisor)
  pid       4242 (supervisor), 4310 (server)
  restarts  1
  last exit 2026-11-16T12:05:00Z (signal 9)
  health    ok
```

### 15.3 Daemon setup and start

```text
$ fleet daemon setup
Pairing code (hidden): ****
machine paired into org_default
Install and start systemd service? [Y/n]: y

=> sudo install -m 0644 /tmp/fleet-daemon.service /etc/systemd/system/fleet-daemon.service
=> sudo systemctl daemon-reload
=> sudo systemctl enable --now fleet-daemon
waiting for daemon health...
daemon installed and running (systemd, scope=user)

$ fleet daemon status
daemon: running (systemd)
  scope     user (User=user, HOME=/home/user)
  unit      fleet-daemon.service active
  paired    yes (~/.fleet/daemon/machine.key.token)
  health    ok (active sessions: 0)
```

### 15.4 Status and doctor

```text
$ fleet status
... both roles ...

$ fleet doctor
PASS  fleet / fleet-admin / fleet-daemon versions match
PASS  admin database and daemon database are distinct
PASS  admin token present (fla_...)
PASS  daemon is paired
PASS  admin health ok
WARN  FLEET_ADMIN_URL overrides ~/.fleet/cli.toml
FAIL  fleet-admin.service and a supervisor state file are both active
      fix: fleet admin stop --mode supervisor
```

## 16. Code architecture

### 16.1 Modules

```text
apps/fleet-cli/src/
  main.rs
  cli.rs                     # top-level Cli, global flags
  commands/
    mod.rs
    admin.rs                 # fleet admin command enum + dispatch
    daemon.rs                # fleet daemon command enum + dispatch
    client.rs                # fleet config, admin REST commands
    network.rs               # fleet network
    status.rs                # fleet status
    doctor.rs                # fleet doctor
  setup/
    admin.rs
    daemon.rs
    client.rs
  process/
    mod.rs                   # mode selection + start/stop/restart/status/logs
    systemd.rs               # systemctl/journalctl delegation, unit rendering
    state.rs                 # supervisor.json, locks, atomic writes
    spawn.rs                 # detached spawn, stdio redirection, setsid
    signals.rs               # process groups, SIGTERM/SIGKILL
    supervisor.rs            # hidden __supervise command and restart loop
    readiness.rs             # admin/daemon health probes
    logs.rs                  # tail, follow, rotation
  config_paths.rs            # role paths, config/db separation checks
  prompt.rs                  # interactive helpers
  pairing.rs                 # low-level pairing helpers
  client.rs                  # AdminClient
  config.rs                  # CliConfig, TomlEdit
  network.rs / connect.rs / cloudflare.rs
  output.rs
  secrets.rs
```

### 16.2 Server crates

```text
apps/fleet-admin/src/lib.rs    pub async fn run(config, options) -> Result<()>
apps/fleet-admin/src/main.rs   thin clap wrapper -> fleet_admin::run

apps/fleet-daemon/src/lib.rs   pub async fn run(config, options) -> Result<()>
apps/fleet-daemon/src/main.rs  thin clap wrapper -> fleet_daemon::run
```

The libraries own the server assembly, migrations, shutdown handling, and
readiness surface. The binaries only parse process-level flags (`--config`,
`serve`) and call the library.

### 16.3 Context split

Do not build an `AdminClient` for local daemon or process commands. Split the
old `AppContext` into:

- `CliSettings`: CLI config path, output mode, role paths.
- `AdminClientContext`: admin URL and token, created only for admin REST
  commands.
- `DaemonContext`: daemon config path, state dir, binary path.
- `ProcessContext`: role, config path, state dir, binary path, mode.

### 16.4 Dependencies

- `fleet-core-config` for `AdminConfig` / `DaemonConfig` parsing and defaults.
- `fleet-core-auth` for `generate_admin_token`, `generate_machine_token`,
  `generate_user_token`, machine identity, and token-file helpers.
- `fleet-core-connection` for `pair_machine`.
- `nix` for `setsid`, `flock`, signals, and process groups (Unix fallback).
- `libc` for `PR_SET_PDEATHSIG` on Linux.
- Existing `clap`, `tokio`, `anyhow`, `serde`, `toml_edit`, `reqwest`.

## 17. Security

- Admin tokens (`fla_`), per-machine tokens (`flm_`), user tokens (`flu_`),
  and pairing codes are never printed by `start`, `status`, `logs`, or the
  fallback supervisor.
- `fleet admin setup` writes the admin token to `0600` config files; the token
  is shown only with `fleet config show --reveal`.
- `fleet daemon setup` uses hidden input for pairing codes and admin tokens.
- The daemon config and CLI config never store the admin token as a daemon
  credential.
- Supervisor state and log directories are `0700`; files are `0600`.
- `stop` refuses to kill a process that fleet did not start.
- System scope runs systemd units as dedicated `fleet-admin` /
  `fleet-daemon` users. User scope runs the unit as the invoking user and reads
  only that user's `~/.fleet` files.
- The admin and daemon database paths must differ; startup fails otherwise.
- Non-loopback binds print a warning recommending Tailscale, Cloudflare, or a
  TLS reverse proxy.
- Non-loopback plaintext `ws://` requires `--allow-insecure`.
- `install-service` and setup use `sudo` with inherited stdio so sudo prompts
  on the controlling terminal. The CLI never captures, pipes, logs, or stores
  the password, and never uses `sudo -S`.
- CLI lifecycle delegation to an already-installed systemd unit does not run
  sudo; it prints the exact privileged command when permissions are missing.
- Unit files are rendered in a private temporary directory (`0600`) and
  installed as `0644`. Environment files are installed as `0600`.
- The fallback supervisor records no secrets in `supervisor.json`.

## 18. Testing

### 18.1 Unit tests

- Command parsing for `fleet admin ...`, `fleet daemon ...`, `fleet setup`,
  `fleet status`, and `fleet doctor`.
- Default path resolution for `~/.fleet/admin`, `~/.fleet/daemon`, and
  `~/.fleet/cli.toml`.
- Database separation and collision rejection.
- Token prefix generation: `fla_`, `flm_`, `flu_`.
- `TomlEdit` integer, boolean, and string values.
- systemd unit rendering and mode selection (`auto`, `systemd`, `supervisor`).
- Unit rendering for both scopes:
  - user scope: `User=<current user>`, `Group=<primary group>`,
    `HOME=<home>`, home config paths;
  - system scope: `User=fleet-admin` / `fleet-daemon`,
    `/etc/fleet` / `/var/lib/fleet` paths.
- systemctl/journalctl command construction (with fake `systemctl` and
  `sudo` binaries in `PATH`).
- Setup install-service flow: prompt, `--install-service`,
  `--no-install-service`, cancelled sudo, missing TTY, missing sudo, and
  `--yes` with/without passwordless sudo.
- Supervisor state read/write, atomic update, stale PID detection.
- PID reuse protection from `/proc/<pid>/cmdline`.
- Log tail and follow behavior.
- Binary discovery order (`--bin`, `FLEET_ADMIN_BIN`/`FLEET_DAEMON_BIN`,
  sibling, `PATH`).
- Admin/daemon setup render valid `AdminConfig` / `DaemonConfig`.
- `fleet doctor` check results for healthy, degraded, and conflicting states.
- `fleet doctor` detects scope mismatches: user-scope unit running as a
  different user, system-scope unit that cannot read `/etc/fleet` or
  `/var/lib/fleet`, and a missing unit when service installation is expected.
- Environment variable rename: no `AGENT_FLEET_*` reads remain.

### 18.2 Integration tests

- `fleet admin setup --install-service` (fake sudo + systemd) writes config,
  installs/enables the unit, and waits for health.
- `fleet daemon setup --install-service` pairs before installing/enabling the
  unit.
- Cancelled sudo, missing TTY, and missing sudo keep the written config and
  print the fallback command.
- `fleet admin setup` + `fleet admin install-service` (fake systemd) +
  `start`/`stop`/`status`/`logs`.
- `fleet admin setup` + `fleet admin start --mode supervisor` +
  `status`/`stop`.
- Kill the fallback server child with `SIGKILL`; supervisor restarts it and
  `status` reports an incremented restart count.
- `fleet daemon setup` in pairing-code mode without an admin token.
- `fleet daemon setup` in admin-token mode.
- `fleet daemon start` + `fleet admin machines list` shows the machine.
- systemd and supervisor conflict detection in `fleet doctor`.
- `fleet admin start --mode auto` does not start a second instance when the
  health endpoint is already reachable.
- `fleet admin stop` refuses to kill an external instance.
- `fleet admin logs -n` returns the expected startup line in supervisor mode.
- `--dry-run` writes nothing and starts nothing.
- Non-TTY without `--yes` fails quickly.
- `fleet status --json` and `fleet doctor --json` are valid JSON and use the
  documented exit codes.

### 18.3 End-to-end script

Add `scripts/test/fleet_e2e.sh`:

1. Use a temporary `HOME`.
2. Build `fleet`, `fleet-admin`, and `fleet-daemon`.
3. `fleet admin setup --yes ...`
4. `fleet admin start --mode supervisor`, wait for health.
5. `fleet config init --yes ...`
6. `fleet daemon setup --yes --code ...` or admin-token mode.
7. `fleet daemon start --mode supervisor`, wait for online.
8. `fleet admin machines list` contains the machine.
9. Kill the admin server child, wait for supervisor restart, assert health.
10. `fleet status` and `fleet doctor`.
11. `fleet daemon stop`, `fleet admin stop`.
12. Assert configs, databases, PID files, and logs are under the expected
    `~/.fleet/...` paths and do not overlap.

systemd integration is validated manually in a VM/container with systemd:
install, `systemctl enable --now`, reboot, verify health, `kill -9` the server
and verify systemd restarts it, then verify `fleet admin status` reports
`systemd`.

## 19. Migration and breaking changes

This is a clean break:

- `af` is removed.
- `agent-admin` is removed; `fleet-admin` replaces it.
- `AGENT_FLEET_*` is removed; `FLEET_*` replaces it.
- `~/.agent-fleet` is ignored; `~/.fleet` is used.
- Token prefixes `af_admin_`, `afm_`, and `afu_` are no longer generated or
  accepted; use `fla_`, `flm_`, and `flu_`.
- `agent-admin.service` / `agent-daemon.service` are replaced by
  `fleet-admin.service` / `fleet-daemon.service`.
- Old `~/.agent-fleet/config.toml`, databases, keys, and tokens are not read.
- The installer does not migrate data. Operators start fresh.

All scripts, docs, E2E tests, packaging files, and examples must be updated in
the same change set.

## 20. Implementation phases

### Phase 0 — Rebrand, binary layout, and command tree

- Rename `af` → `fleet`; `agent-admin` → `fleet-admin`.
- Add `src/lib.rs` to `apps/fleet-admin` and `apps/fleet-daemon`; make the
  binaries thin entrypoints.
- Rename token prefixes to `fla_`, `flm_`, and `flu_`.
- Rename `AGENT_FLEET_*` → `FLEET_*`.
- Replace `~/.agent-fleet` with `~/.fleet`; rename databases to
  `fleet-admin.db` and `fleet-daemon.db`.
- Introduce `fleet admin`, `fleet daemon`, `fleet config`, `fleet network`,
  `fleet setup`.
- Move existing commands into the new namespaces.
- Update packaging, installer, E2E, docs, and examples.

Acceptance: all existing functionality works through the new names; no
`af`/`agent-admin`/`AGENT_FLEET_*`/`~/.agent-fleet` references remain.

### Phase 1 — Setup, separated configuration, status/doctor skeleton

- `fleet admin setup`, `fleet daemon setup`, `fleet config init`, `fleet setup`.
- Separate config and database paths.
- DB collision guard.
- Pairing code and admin-token daemon setup modes.
- `install-service` / `uninstall-service` unit rendering for user and system
  scope.
- Setup auto-install prompt, `--install-service` / `--no-install-service`,
  sudo-with-inherited-stdio, and the skip/print-command fallback.
- `fleet status` and `fleet doctor` core checks.
- `--yes` / `--dry-run`.

Acceptance: a clean host can set up and pair admin + daemon with separated
files and databases; setup can install and enable the systemd units; cancelled
sudo leaves a working config and prints the manual command; `status` and
`doctor` report the expected state.

### Phase 2 — systemd primary lifecycle

- `start`/`stop`/`restart`/`status`/`logs` delegation to
  `systemctl`/`journalctl`.
- `run` foreground mode for ExecStart.
- Mode detection (`auto`, `systemd`).
- Real systemd units, boot autostart, crash restart, and scope-aware status.
- `fleet admin start` and `fleet daemon start` use the installed unit by
  default.

Acceptance: `fleet admin start` starts the systemd unit; `kill -9` triggers a
systemd restart; `fleet admin status` reports systemd mode; reboot autostarts
both services.

### Phase 3 — Fallback supervisor

- Hidden `fleet __supervise` process.
- Detached spawn, state files, process groups, backoff restart, log rotation.
- `--mode supervisor` and `auto` fallback.
- Conflict detection between systemd and supervisor modes.

Acceptance: on a host without systemd, `fleet admin start` keeps the server
alive across `kill -9`; `stop` leaves no processes behind.

### Phase 4 — Tests, docs, and polish

- E2E script.
- README, development, connectivity, security, crate interface, docs site.
- Full `fleet status` / `fleet doctor` output and JSON.
- Manual systemd verification.

Acceptance: CI green; manual systemd verification documented.

## 21. Acceptance criteria

- Three binaries exist: `fleet`, `fleet-admin`, `fleet-daemon`; server logic is
  in library crates and the server binaries are thin.
- Admin and daemon config files and databases are separate and cannot be
  configured to the same database.
- Token prefixes are `fla_`, `flm_`, and `flu_`.
- All environment variables are `FLEET_*`.
- `fleet setup` forwards to the role setup commands.
- `fleet admin setup` and `fleet daemon setup` work interactively and with
  `--yes`.
- `fleet admin setup` and `fleet daemon setup` offer to install and start the
  systemd unit by default; `--install-service` / `--no-install-service`
  control it non-interactively.
- Sudo installation prompts through sudo with inherited stdio; the CLI never
  captures the password. Cancelled sudo, no TTY, or missing sudo leaves setup
  successful and prints the exact command.
- User scope installs a system unit that runs as the current user and reads
  `~/.fleet`; system scope uses `/etc/fleet` + `/var/lib/fleet` and the
  dedicated `fleet-admin` / `fleet-daemon` users.
- `fleet admin install-service` and `fleet daemon install-service` install and
  enable systemd units.
- systemd units restart on crash and start at boot.
- `fleet admin start|stop|restart|status|logs` delegate to systemd in
  systemd mode.
- `fleet admin start --mode supervisor` provides crash restart on hosts without
  systemd.
- `fleet status` reports both roles and their active mode.
- `fleet doctor` reports PASS/WARN/FAIL and exits non-zero on FAIL.
- `fleet network` updates the separated admin and daemon configs.
- No `af`, `agent-admin`, `AGENT_FLEET_*`, `af_admin_`, `afm_`, `afu_`, or
  `~/.agent-fleet` references remain in code, scripts, packaging, or docs.

## 22. Future work

- Windows supervisor/service support.
- macOS launchd packaging and `fleet admin install-service --launchd`.
- True `systemctl --user` units with `loginctl enable-linger`, as an
  alternative to the user-scope system unit.
- Optional `fleet doctor --fix` for safe, explicit repairs.
- Multiple admin instances per host.

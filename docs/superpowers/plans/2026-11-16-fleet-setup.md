# Fleet Setup, Configuration, and Service Installation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement `fleet setup`, `fleet admin setup`, `fleet daemon setup`, role config commands, systemd install/uninstall with sudo, `fleet admin run`/`fleet daemon run`, and local daemon commands.

**Architecture:** Role paths and scopes are resolved centrally. Configuration is written with typed TOML edits. Systemd units use `fleet ADMIN|daemon run` and are rendered from the shared packaging templates. Setup prompts interactively and uses `sudo` with inherited stdio; failed sudo leaves written configuration and prints runnable commands.

**Tech Stack:** Rust workspace, clap, tokio, anyhow, toml/toml_edit, rpassword, libc (Unix euid), systemd, bash.

---

## Scope

Plan 1 is committed as `695ccc1`. This plan implements:

- `fleet setup` dispatcher.
- `fleet admin setup` and `fleet daemon setup`.
- `fleet admin config show|path|validate`, `fleet daemon config show|path|validate`.
- `fleet admin install-service|uninstall-service`, `fleet daemon install-service|uninstall-service`.
- `fleet admin run`, `fleet daemon run` (foreground exec used by units).
- `fleet daemon pair|providers|workspace add|list|remove|rotate-key|identity show`.
- DB collision guard and role-separated paths.

Not in this plan:

- `start`/`stop`/`restart`/`status`/`logs` and the fallback supervisor → Plan 3.
- `fleet status` and `fleet doctor` → Plan 4.
- Full status/doctor output and final E2E polish → Plan 4.
- `fleet ... run` in this plan is an exec shim for systemd; config/DB
  validation, health preflight, and conflict checks for `run` are Plan 3.
  The shim still loads the role config and checks DB separation.

## Execution order

Task 1 is committed (`3706ac6`). Tasks 2 and 3 can run in parallel (disjoint
files). Tasks 4-8 share `cli.rs`/`main.rs`/`setup/mod.rs` and must run
sequentially: 4 → 5 → 6 → 7 → 8. Task 9 is last. Do not run 4-8 in parallel.

## Global constraints

- Build/test with:
  ```bash
  CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo <args>
  ```
- `fleet` must not depend on `fleet-admin` or `fleet-daemon` crates.
- Unit templates are shared by the installer and CLI through `include_str!`.
- Tests never execute `sudo`, `systemctl`, `useradd`, or write under `/etc`/`/var/lib`.
- `--dry-run` writes nothing and executes nothing.
- `--yes` is non-interactive.
- Secrets are `0600`; CLI-created role/staging directories are `0700`.
- System-scope **setup** is root-only. Standalone `install-service --system` may use sudo.
- Commits end with `Co-Authored-By: Claude Code <noreply@anthropic.com>`.

## Authoritative design decisions

### D1. Role paths

`apps/fleet-cli/src/config_paths.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role { Admin, Daemon }

impl Role {
    pub fn as_str(self) -> &'static str;
    pub fn binary_name(self) -> &'static str; // fleet-admin / fleet-daemon
    pub fn unit_name(self) -> &'static str;   // fleet-admin.service / ...
    pub fn bin_env(self) -> &'static str;     // FLEET_ADMIN_BIN / FLEET_DAEMON_BIN
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope { User, System }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeRequest { Auto, User, System }

#[derive(Debug, Clone, Default)]
pub struct RolePathOverrides {
    pub config: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub database: Option<PathBuf>,
    pub bin: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolePaths {
    pub role: Role,
    pub scope: Scope,
    pub config: PathBuf,
    pub state_dir: PathBuf,
    pub database: PathBuf,
    pub binary: Option<PathBuf>,
    pub fleet_binary: PathBuf,
    pub service_user: String,
    pub service_group: String,
    pub env_file: Option<PathBuf>,
    pub web_dir: Option<PathBuf>,
}
```

`resolve` discovers the server binary; `resolve_config` leaves `binary = None`;
`resolve_with` is a test helper that uses `home/.fleet/...` defaults.

```rust
impl RolePaths {
    pub fn resolve(role: Role, request: ScopeRequest, o: &RolePathOverrides) -> Result<Self> {
        let scope = resolve_scope(request, is_root());
        Self::resolve_inner(role, scope, o, &current_user(), &current_group(), &home_dir(), true, true)
    }

    pub fn resolve_config(role: Role, request: ScopeRequest, o: &RolePathOverrides) -> Result<Self> {
        let scope = resolve_scope(request, is_root());
        Self::resolve_inner(role, scope, o, &current_user(), &current_group(), &home_dir(), false, true)
    }

    #[cfg(test)]
    pub fn resolve_with(
        role: Role, scope: Scope, o: &RolePathOverrides,
        user: &str, group: &str, home: &Path,
    ) -> Result<Self> {
        // Path tests must not depend on `fleet-admin`/`fleet-daemon` being
        // installed; discovery gets its own test with an explicit override.
        Self::resolve_inner(role, scope, o, user, group, home, false, false)
    }

    fn resolve_inner(
        role: Role, scope: Scope, o: &RolePathOverrides,
        user: &str, group: &str, home: &Path,
        discover: bool, core_defaults: bool,
    ) -> Result<Self> { /* see below */ }

    pub fn require_binary(&self) -> Result<&Path>;
    pub fn is_system(&self) -> bool;
}
```

`resolve_inner` defaults:

| Role/scope | config | state | service user/group | env file |
|---|---|---|---|---|
| admin/user | `fleet_core_config::default_admin_config_path()` or `home/.fleet/admin/config.toml` | `~/.fleet/admin` | current user/group | none |
| daemon/user | `fleet_core_config::default_daemon_config_path()` or `home/.fleet/daemon/config.toml` | `~/.fleet/daemon` | current user/group | none |
| admin/system | `/etc/fleet/admin.toml` | `/var/lib/fleet/admin` | `fleet-admin` | `/etc/fleet/admin.env` |
| daemon/system | `/etc/fleet/daemon.toml` | `/var/lib/fleet/daemon` | `fleet-daemon` | `/etc/fleet/daemon.env` |

- `--config`, `--state-dir`, `--database`, `--bin` override the corresponding values.
- User-scope `web_dir` is `apps/web/dist` when it exists; system admin is `/usr/local/share/fleet/web`.
- `fleet_binary` is `std::env::current_exe()?`.
- `discover_binary` order: override → `FLEET_*_BIN` → sibling of `fleet` → `PATH`; error lists searched paths.
- `current_user` prefers `SUDO_USER`; `current_group` runs `id -gn <user>` and falls back to the user name.
- `ensure_distinct_databases(admin, daemon)` expands `~`, canonicalizes existing paths, and rejects equality.

Tests: scope defaults, user/system paths, overrides, collision, `resolve_config` has `binary == None`, discovery override.

### D2. CLI context and role flags

In `cli.rs`:

```rust
#[derive(Debug, Clone, Args, Default)]
pub struct RoleArgs {
    #[arg(long, global = true)] pub config: Option<PathBuf>,
    #[arg(long, global = true)] pub state_dir: Option<PathBuf>,
    #[arg(long, global = true)] pub bin: Option<PathBuf>,
    #[arg(long, global = true)] pub system: bool,
    #[arg(long, global = true)] pub user: bool,
}

impl RoleArgs {
    pub fn scope_request(&self) -> Result<ScopeRequest>;
    pub fn paths(&self, role: Role, database: Option<PathBuf>) -> Result<RolePaths>;
    pub fn config_paths(&self, role: Role, database: Option<PathBuf>) -> Result<RolePaths>;
    pub fn setup_paths(&self, role: Role, database: Option<PathBuf>, prompt: PromptOptions) -> Result<RolePaths>;
}
```

- `Command::Admin { #[command(flatten)] role: RoleArgs, #[command(subcommand)] action }`
- `Command::Daemon { ... }` similarly.
- `setup_paths` prompts for scope when `Auto` and interactive; bails if
  system scope and not root.
- `paths` discovers the server binary; `config_paths` calls `resolve_config`.

`AppContext`:

```rust
struct AppContext {
    token: Option<String>,
    json: bool,
    config: CliConfig,
    config_path: PathBuf,
    admin_url: String,
    client: std::sync::OnceLock<AdminClient>,
}

impl AppContext {
    fn client(&self) -> Result<&AdminClient> {
        if let Some(client) = self.client.get() { return Ok(client); }
        let client = AdminClient::new(&self.admin_url, self.token.clone())?;
        let _ = self.client.set(client);
        self.client.get().ok_or_else(|| anyhow::anyhow!("failed to initialize admin client"))
    }
}
```

`run()` no longer constructs `AdminClient`. All call sites use `ctx.client()?`.
`test_context` uses `client: std::sync::OnceLock::new()`.

### D3. Typed TOML edits

`config.rs`:

```rust
#[derive(Debug, Clone)]
pub enum TomlValue { String(String), Integer(i64), Boolean(bool) }

#[derive(Debug, Clone)]
pub struct TomlEdit { pub section: &'static str, pub key: &'static str, pub value: TomlValue }

impl TomlEdit {
    pub fn string(section: &'static str, key: &'static str, value: impl Into<String>) -> Self;
    pub fn integer(section: &'static str, key: &'static str, value: i64) -> Self;
    pub fn boolean(section: &'static str, key: &'static str, value: bool) -> Self;
}
```

`edit_toml_with_removals` stores the typed `toml_edit::value`. Update all
existing struct literals, including `network.rs` (two sites) and `pairing.rs`.
Add `create_private_dir(path)` (`0700` on Unix); `CliConfig::save` uses it for
the parent.

### D4. Prompt helpers

`prompt.rs`:

```rust
#[derive(Debug, Clone, Copy)]
pub struct PromptOptions { pub yes: bool, pub dry_run: bool }

impl PromptOptions {
    pub fn interactive(&self) -> bool { !self.yes && stdin.is_terminal() && stdout.is_terminal() }
}

pub fn input(o: PromptOptions, question: &str, default: &str) -> Result<String>;
pub fn confirm(o: PromptOptions, question: &str, default: bool) -> Result<bool>;
pub fn select(o: PromptOptions, question: &str, choices: &[&str], default: usize) -> Result<usize>;
pub fn secret(question: &str) -> Result<String>; // rpassword
fn parse_choice(input: &str, len: usize, default: usize) -> Option<usize>;
fn parse_yes_no(input: &str, default: bool) -> Option<bool>;
```

Retain a `confirm(message, yes)` wrapper in `cli.rs` that calls
`prompt::confirm(..., false)` and bails when it returns false. Keep every
existing delete/rotate call site working.

Tests: `parse_choice("3", 3, 0) == Some(2)`, `parse_choice("4", 3, 0) == None`,
yes/no defaults.

### D5. `fleet ... run` and systemd

`process/mod.rs`:

```rust
pub mod systemd;

pub fn run_role(paths: &RolePaths) -> Result<()> {
    let binary = paths.require_binary()?;
    let mut command = std::process::Command::new(binary);
    command.arg("--config").arg(&paths.config).arg("serve");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = CommandExt::exec(&mut command);
        anyhow::bail!("failed to exec {}: {error}", binary.display());
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        if !status.success() { anyhow::bail!("{} exited with {status}", binary.display()); }
        Ok(())
    }
}
```

`AdminCommand::Run` / `DaemonCommand::Run` call `crate::process::run_role`.
`local.rs` (Task 7) reuses it; Task 4 owns `process/mod.rs`.

Templates:

```ini
[Unit]
Description=fleet admin (REST API + web UI)
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=60
StartLimitBurst=10

[Service]
Type=simple
User=@ADMIN_USER@
Group=@ADMIN_GROUP@
Environment=HOME=@ADMIN_HOME@
EnvironmentFile=-@ENV_FILE@
ExecStart=@FLEET_BIN@ admin run --config @CONFIG_FILE@
Restart=always
RestartSec=2
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=@ADMIN_STATE_DIR@
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

Daemon template is analogous with `@DAEMON_*@`, `@FLEET_BIN@ daemon run`,
no `ProtectHome`.

- Installer substitutes `@FLEET_BIN@="$BIN_DIR/fleet"`, `@CONFIG_FILE@`,
  `@ENV_FILE@`.
- User scope: `User/Group=current`, `Environment=HOME=<home>`, no
  `EnvironmentFile` line, no `ProtectHome` line.
- System scope: dedicated users; `HOME=<state dir>`.
- `install_check.sh` must expect `fleet admin run` / `fleet daemon run` for both
  default and explicit-prefix installs.

### D6. Service installation

`process/systemd.rs`:

```rust
pub fn render_unit(paths: &RolePaths) -> Result<String>;
pub fn systemd_available() -> bool; // linux + systemctl --version + /run/systemd/system
pub fn sudo_required(is_root: bool, yes: bool, sudo_n_ok: bool, tty: bool) -> bool;

pub struct CommandSpec { pub program: String, pub args: Vec<String>, pub privileged: bool }
pub struct ServicePlan {
    pub unit_name: String,
    pub unit_path: PathBuf,
    pub unit: String,
    pub env_path: Option<PathBuf>,
    pub env: Option<String>,
    pub commands: Vec<CommandSpec>,
}

pub fn plan_install(
    paths: &RolePaths,
    admin_token: Option<&str>,
    start: bool,
    user_exists: bool,
    env_exists: bool,
) -> Result<ServicePlan>;

pub fn install(
    paths: &RolePaths,
    admin_token: Option<&str>,
    start: bool,
    dry_run: bool,
    yes: bool,
) -> Result<InstallOutcome>;
```

Semantics:

- `HOME` is derived internally: `paths.state_dir` for system scope, otherwise
  `crate::config::home_dir()`. Callers never pass a home directory.
- Admin env file is written only if `env_exists == false` and a token is
  available. If the token rotates and the env file exists, update
  `FLEET_ADMIN_TOKEN` in place (or print the required change).
- Daemon env file is created empty only if missing.
- `useradd` is emitted only when `!user_exists`; production checks `id -u`.
- `start` chooses `systemctl enable --now` vs `systemctl enable`.
- `install` writes staging files under `~/.fleet/staging/<role>/` with `0600`
  (dir `0700`); skipped-sudo output uses those real paths. Dry-run writes
  nothing and prints the unit plus logical commands.
- System scope adds `chown -R <user>:<group> <state dir>` and config mode
  `0640` ownership.
- `run_command` skips when not root, no TTY, and `yes == false`; with `yes`,
  requires `sudo -n true`. Never captures a password.
- `ServiceArgs { yes: bool, dry_run: bool }`; namespace `RoleArgs` owns
  `--system`/`--user`.

Tests: user/system rendering, user scope filters `EnvironmentFile`/
`ProtectHome`, `sudo_required` matrix, plan commands, dry-run no writes.

### D7. Admin setup

`setup/admin.rs`:

```rust
pub struct AdminSetupOptions {
    pub paths: RolePaths,
    pub bind: Option<String>,
    pub database: Option<PathBuf>,
    pub web_dir: Option<PathBuf>,
    pub no_web: bool,
    pub rotate_admin_token: bool,
    pub write_cli_config: bool,
    pub install_service: Option<bool>,
    pub start: bool,
    pub prompt: PromptOptions,
    pub cli_config_path: PathBuf,
}
```

`build_plan` has no side effects and does not call `resolve_paths`.

Database precedence for both roles: `--database` → the loaded config value when
the config file existed or `FLEET_DB`/`FLEET_DATA_DIR` is set → `paths.database`.
Key precedence: `--key-file` → the loaded config value when the config file
existed or `FLEET_DATA_DIR` is set → `paths.state_dir/machine.key`. This keeps
new system-scope configs out of `~/.fleet` while honoring explicit env
overrides.

`--start` on setup is accepted. After service installation it is a no-op
(the unit was enabled with `--now`); when installation is skipped it prints
the Plan 3 `fleet <role> start` command.

Interactive flow:

1. Scope is resolved by `RoleArgs::setup_paths` (prompts user/system).
2. Bind prompt (default existing host:port).
3. Web dir prompt (default existing static dir or `apps/web/dist`; empty = no web).
4. Token prompt: generate or reuse; `--rotate-admin-token` forces generation.
5. Write CLI config prompt (default yes).
6. Install-service prompt (default `systemd_available()`).
7. `--yes` uses the defaults above.

Writes:

- Admin config via typed edits; when `--no-web`, remove `[web] static_dir`.
- State dir with `create_private_dir` (user scope) before service install.
- CLI config with `admin_url`, token, `admin_config`, `daemon_config`.
- DB collision check against the daemon config from `cli.toml` (or the default).
- Service install with `start = true`, passing the admin token for the env file.
- After install, poll health for ~15s; on failure print the systemctl hint.

System scope is root-only; `setup_paths` enforces it before writes.
Dry-run prints the plan and returns before any write or directory creation.

Tests: `fla_` token, DB collision with a temporary daemon TOML, dry-run writes
nothing, `--no-web` removes static_dir.

### D8. Daemon setup and pairing

`setup/daemon.rs` defines `DaemonSetupArgs` once (clap `Args`) and is used by
`cli.rs` as `DaemonCommand::Setup(crate::setup::daemon::DaemonSetupArgs)`.

Fields: `org`, `server`, `code`, `name`, `key_file`, `database`, `workspace`
(Vec), `store_token`, `configure_only`, `allow_insecure`, `install_service`,
`no_install_service`, `start`, `yes`, `dry_run`.

Dispatch passes one concrete options type:

```rust
pub struct DaemonSetupOptions {
    pub paths: RolePaths,
    pub args: DaemonSetupArgs,
    pub admin_url: String,
    pub admin_token: Option<String>,
    pub prompt: PromptOptions,
    pub cli_config_path: PathBuf, // ctx.config_path
}
```

Mode selection:

```rust
pub enum Mode { Code(String), AdminToken(String), ConfigureOnly }

pub fn choose_mode(
    args: &DaemonSetupArgs,
    admin_token: Option<String>,
    prompt: PromptOptions,
) -> Result<Mode>;
```

- `--configure-only` → ConfigureOnly.
- `--code` → Code.
- Interactive and no mode flag → prompt:
  `1) Redeem a pairing code`, `2) Use an admin token`, `3) Configure only`.
  Code/admin token use `prompt::secret`.
- `--yes` with no code/token/configure-only → error.

`run`:

1. Remember whether the daemon config existed.
2. Load config (no `resolve_paths` yet).
3. DB/key defaults:
   - `--database` if set;
   - else the loaded config value when the file existed;
   - else `paths.database`.
   - `--key-file` if set; else loaded key when the file existed; else
     `paths.state_dir/machine.key`.
   - expand `~`.
4. Admin config path from `cli.toml` or default; DB collision check.
5. Derive/validate `server_url`.
6. Dry-run: print and return before mode validation, identity creation, or
   any write.
7. Choose/validate the mode. `--yes` with no code, token, or configure-only
   errors here, before creating key/id files.
8. `create_private_dir` for key/db parents.
9. Load/create identity and machine id.
10. Pair (code/admin-token) or configure-only.
11. Write daemon config with typed edits; remove `connection.token` and
    `connection.pairing_code`.
12. Optional `--store-token` writes the admin token to CLI config.
13. Optional `--workspace` calls a private `run_daemon_json` helper in this
    module; do not depend on `local.rs`.
14. Service install:
    - `start = token_present`;
    - configure-only with no token installs the unit with `start = false`;
    - default install is `systemd_available()`.
15. Print next steps (`fleet daemon pair` when unpaired, otherwise start hint).

`pairing.rs` adds `pair_with_code` and `pair_only`, and removes the old
`setup`/`SetupOptions`/`SetupMode` once the new flow is wired. `pair_only`
writes only `<key_file>.token`.

Tests: mode selection, configure-only config writes/removals, dry-run writes
nothing, DB collision, system-scope new-config paths.

### D9. Config and local commands

`config_commands.rs` uses `RoleArgs::config_paths` (no server binary):

- `admin show [--reveal]`, `admin path`, `admin validate`.
- `daemon show`, `daemon path`, `daemon validate`.
- Use `expand_tilde` for display; do not call `resolve_paths`.
- Validate needs the other role's config path. Pass it from
  `AppContext`/`CliConfig`: `config.admin_config` / `config.daemon_config`,
  falling back to `default_admin_config_path()` / `default_daemon_config_path()`.
  The function signature is:
  ```rust
  pub fn admin_validate(paths: &RolePaths, daemon_config: &Path) -> Result<()>;
  pub fn daemon_validate(paths: &RolePaths, admin_config: &Path) -> Result<()>;
  ```
- `fleet daemon pair` and `fleet daemon identity show` use `RoleArgs::config_paths`
  too; they must work without a `fleet-daemon` binary.

`local.rs`:

```rust
pub struct DaemonCommand { pub program: PathBuf, pub args: Vec<String> }
impl DaemonCommand {
    pub fn new(paths: &RolePaths, args: Vec<String>) -> Result<Self>;
}
pub async fn run_daemon(args: &[String], paths: &RolePaths) -> Result<()>;
pub fn run_role(paths: &RolePaths) -> Result<()>;
pub fn identity_json(paths: &RolePaths) -> Result<serde_json::Value>;
```

- `identity_json` is read-only: `MachineIdentity::load` only if the key exists;
  read `machine_id_path` directly; never create key/id/token. Missing key →
  `fingerprint: null`, `paired` from token-file presence.
- `workspace remove` passes `--yes` to the daemon.
- `providers`: `fleet-daemon ... providers`.
- `workspace add`: `fleet-daemon ... register-workspace`.
- `workspace list/remove`: new daemon subcommands
  `list-workspaces` / `remove-workspace`.
- `rotate-key`: `fleet-daemon ... rotate-key`.

Daemon additions in `bootstrap.rs`:

```rust
pub async fn list_workspaces(config_path: Option<PathBuf>) -> anyhow::Result<()>;
pub async fn remove_workspace(config_path: Option<PathBuf>, id: String, yes: bool) -> anyhow::Result<()>;
```

and `Command::ListWorkspaces`, `Command::RemoveWorkspace { id, yes }` in
`lib.rs`.

### D10. `fleet setup` dispatcher

`setup/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRole { Admin, Daemon, Cli }

pub fn child_args(role: SetupRole, yes: bool, args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = match role {
        SetupRole::Admin => vec!["admin".into(), "setup".into()],
        SetupRole::Daemon => vec!["daemon".into(), "setup".into()],
        SetupRole::Cli => vec!["config".into(), "init".into()],
    };
    if yes && role != SetupRole::Cli { out.push("--yes".into()); }
    out.extend(args.iter().cloned());
    out
}

pub async fn dispatch(role: Option<SetupRole>, yes: bool, args: Vec<String>) -> Result<()>;
```

`Command::Setup { role: Option<SetupRoleArg>, yes: bool, args: Vec<String> }`
with `trailing_var_arg = true, allow_hyphen_values = true`. `dispatch` prompts
for a role when absent, prints the child command, and re-execs the current
`fleet` executable with inherited stdio.

---

## File map

**CLI**
- Modify: `apps/fleet-cli/Cargo.toml`
- Modify: `apps/fleet-cli/src/main.rs`
- Modify: `apps/fleet-cli/src/cli.rs`
- Modify: `apps/fleet-cli/src/config.rs`
- Modify: `apps/fleet-cli/src/pairing.rs`
- Modify: `apps/fleet-cli/src/network.rs`
- Create: `apps/fleet-cli/src/config_paths.rs`
- Create: `apps/fleet-cli/src/prompt.rs`
- Create: `apps/fleet-cli/src/process/mod.rs` (Task 4 adds `run_role`)
- Create: `apps/fleet-cli/src/process/systemd.rs`
- Create: `apps/fleet-cli/src/setup/mod.rs`
- Create: `apps/fleet-cli/src/setup/admin.rs`
- Create: `apps/fleet-cli/src/setup/daemon.rs`
- Create: `apps/fleet-cli/src/config_commands.rs`
- Create: `apps/fleet-cli/src/local.rs`

**Daemon local commands**
- Modify: `apps/fleet-daemon/src/lib.rs`
- Modify: `apps/fleet-daemon/src/bootstrap.rs`

**Packaging/tests/docs**
- Modify: `packaging/systemd/fleet-admin.service.in`
- Modify: `packaging/systemd/fleet-daemon.service.in`
- Modify: `scripts/install/install.sh`
- Modify: `scripts/test/install_check.sh`
- Create: `scripts/test/setup_e2e.sh`
- Modify: `package.json`
- Modify: `README.md`, `docs/development.md`, `docs/connectivity.md`,
  `docs/security.md`, `docs/crate-interfaces.md`
- Modify: `apps/docs/src/content/docs/**`

---

## Task 1: Role paths, scopes, lazy client

**Files:** `Cargo.toml`, `main.rs`, `cli.rs`, `config_paths.rs`, `config.rs`.

- [ ] Add `libc` target dep and move `tempfile` to normal dependencies.
- [ ] Create `config_paths.rs` per D1; add tests.
- [ ] Add `create_private_dir` and use it in `CliConfig::save`.
- [ ] Add `RoleArgs` and flatten it into `Command::Admin`/`Command::Daemon`.
- [ ] Replace `AppContext.client` with a `OnceLock` and `client()` method;
  update every call site and `test_context`.
- [ ] Remove eager `AdminClient::new` from `run()`.
- [ ] Add `mod config_paths;` to `main.rs`.
- [ ] Run:
  ```bash
  CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli
  CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo build -p fleet-cli --bin fleet
  ```
- [ ] Commit: `feat(cli): add role paths, scopes, and lazy admin client`.

## Task 2: Typed TOML edits

**Files:** `config.rs`, `pairing.rs`, `network.rs`.

- [ ] Add `TomlValue`, typed `TomlEdit`, constructors, and typed editing.
- [ ] Update `network.rs` and `pairing.rs` literals.
- [ ] Test integer/boolean edits and preserve existing string edits.
- [ ] Run `cargo test -p fleet-cli`.
- [ ] Commit: `refactor(cli): support typed TOML edits`.

## Task 3: Prompt helpers

**Files:** `prompt.rs`, `main.rs`, `cli.rs`.

- [ ] Implement D4 and `mod prompt;`.
- [ ] Keep a `confirm(message, yes)` wrapper; keep all existing call sites.
- [ ] Add `RoleArgs::setup_paths` (prompt scope; root-only system scope).
- [ ] Tests: choice parsing, yes/no defaults.
- [ ] Run `cargo test -p fleet-cli`.
- [ ] Commit: `feat(cli): add interactive prompt helpers`.

## Task 4: systemd, `run`, installer

**Files:** templates, `install.sh`, `install_check.sh`, `process/*`,
`main.rs`, `cli.rs`.

- [ ] Update templates per D5; `StartLimit*` in `[Unit]`.
- [ ] Update installer substitutions: `@FLEET_BIN@`, `@CONFIG_FILE@`,
  `@ENV_FILE@`.
- [ ] Update `install_check.sh`, including explicit-prefix `ExecStart` checks.
- [ ] Implement `fleet admin run`/`fleet daemon run` via `run_role`.
- [ ] Implement D6 rendering/planning/install/uninstall; tests for rendering,
  sudo gating, dry-run.
- [ ] Remove `--system`/`--user` from `ServiceArgs`.
- [ ] Run `cargo test -p fleet-cli`, `./scripts/test/install_check.sh`.
- [ ] Commit: `feat(cli): render and install systemd units`.

## Task 5: Admin setup

**Files:** `setup/admin.rs`, `setup/mod.rs`, `cli.rs`, `main.rs`.

- [ ] `setup/mod.rs` initially contains only `pub mod admin;`.
- [ ] Implement D7, including interactive prompts, dry-run no-write, typed
  config edits, `--no-web` removal, CLI config write, DB guard, service
  install with token and `start = true`.
- [ ] Tests: token, collision (temporary daemon TOML), dry-run, no-web.
- [ ] Run `cargo test -p fleet-cli`.
- [ ] Commit: `feat(cli): implement fleet admin setup`.

## Task 6: Daemon setup and pairing

**Files:** `setup/daemon.rs`, `setup/mod.rs`, `pairing.rs`, `cli.rs`, `main.rs`.

- [ ] `setup/mod.rs` adds `pub mod daemon;`.
- [ ] Implement D8; define `DaemonSetupArgs` once.
- [ ] Implement `pair_with_code`/`pair_only`; remove old setup types.
- [ ] Private `run_daemon_json` in `setup/daemon.rs`.
- [ ] Tests: mode selection, configure-only writes/removals, dry-run,
  DB collision, system-scope new config paths.
- [ ] Run `cargo test -p fleet-cli`.
- [ ] Commit: `feat(cli): implement fleet daemon setup and pairing modes`.

## Task 7: Config and local commands

**Files:** `config_commands.rs`, `local.rs`, `main.rs`, `cli.rs`,
daemon `lib.rs`/`bootstrap.rs`.

- [ ] Implement D9; config commands use `config_paths`.
- [ ] Add daemon list/remove workspace subcommands.
- [ ] Read-only `identity_json`.
- [ ] Tests: daemon command construction, identity missing-key behavior.
- [ ] Run `cargo test -p fleet-cli`, `cargo test -p fleet-daemon`.
- [ ] Commit: `feat(cli): add role config and daemon local commands`.

## Task 8: `fleet setup` dispatcher

**Files:** `setup/mod.rs`, `cli.rs`, `main.rs`.

- [ ] Implement D10; `--yes` forwarded only for admin/daemon.
- [ ] Tests: child args, parser.
- [ ] Run `cargo test -p fleet-cli`.
- [ ] Commit: `feat(cli): add interactive fleet setup dispatcher`.

## Task 9: E2E, docs, verification

**Files:** `scripts/test/setup_e2e.sh`, `package.json`, docs.

- [ ] Create `setup_e2e.sh` with a temporary `HOME`, `FLEET_CLI_CONFIG`,
  `--state-dir` for both roles, and unset ambient `FLEET_*` overrides.
- [ ] Both setup calls pass `--yes --no-install-service` so the test never
  attempts sudo/systemctl/useradd.
- [ ] Test admin setup, direct admin start, configure-only daemon setup,
  config validate, identity show (`paired: false`).
- [ ] Update docs for setup flags, scopes, sudo fallback, run/pair/config
  commands, and Plan 3/4 deferrals.
- [ ] Run:
  ```bash
  cargo fmt --all
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  pnpm run test:web
  ./scripts/test/install_check.sh
  ./scripts/test/setup_e2e.sh
  ```
- [ ] Final legacy-name scan (the three marked installer cleanup lines are
  allowed).
- [ ] Commit: `test: cover fleet setup and service configuration`.

---

## Acceptance checklist

- [ ] `fleet setup` prompts/forwards to admin, daemon, or CLI setup.
- [ ] Admin setup writes `0600` config with `fla_` token, optional web dir,
  CLI config, and DB collision guard.
- [ ] Daemon setup supports code/admin-token/configure-only, writes config,
  saves `<key_file>.token`, removes legacy keys.
- [ ] DB paths are distinct and role-separated.
- [ ] Config show/path/validate work without server binaries.
- [ ] `fleet daemon pair|providers|workspace|rotate-key|identity` work
  without an AdminClient.
- [ ] Units use `fleet admin|daemon run`, restart always, start at boot, and
  install for user/system scope.
- [ ] Sudo uses inherited stdio; skipped installation prints runnable commands.
- [ ] `--dry-run` writes/executes nothing; `--yes` is non-interactive.
- [ ] Workspace tests, clippy, web checks, installer check, and setup E2E pass.

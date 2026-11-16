# Fleet Rebrand and Command Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the CLI and server binaries, rebrand tokens/environment/paths, and move the existing command surface into `fleet admin` / `fleet daemon` while keeping all current behavior working.

**Architecture:** Three binaries remain: `fleet` (thin CLI), `fleet-admin` (admin server), and `fleet-daemon` (daemon server). Server logic moves into library crates with thin binary entrypoints. This plan performs the mechanical rebrand and command-tree split; process lifecycle, setup expansion, and status/doctor behavior arrive in later plans.

**Tech Stack:** Rust workspace, clap, tokio, toml/toml_edit, SQLite, axum, tokio-tungstenite, Ed25519, bash packaging scripts.

---

## Scope check and roadmap

The design spec covers four independently testable stages:

| Plan | Scope | Produces |
|---|---|---|
| **1 (this plan)** | Rebrand, three-binary/lib layout, token/env/path rebrand, command tree | Existing behavior works through the new names |
| 2 | `fleet admin setup`, `fleet daemon setup`, `fleet config`, `fleet setup`, separated configs/DBs, `install-service`, sudo handling | A clean host can set up and install services |
| 3 | `start`/`stop`/`restart`/`status`/`logs`/`run`, systemd delegation, fallback supervisor | CLI-managed and systemd-managed lifecycle |
| 4 | `fleet status`, `fleet doctor`, installer/packaging finalization, E2E/docs | Production-ready operations and diagnostics |

Do not implement Plan 2–4 behavior in this plan. New commands that belong to later plans must parse correctly and return a clear "implemented in a later plan" error.

## Global constraints

- No backward compatibility. Remove `af`, `agent-admin`, `AGENT_FLEET_*`, `af_admin_`, `afm_`, `afu_`, and `~/.agent-fleet` from code, scripts, tests, packaging, and docs. Historical design documents under `docs/superpowers/specs` and `docs/superpowers/plans` may keep old references.
- One documented exception: the installer's one-time legacy cleanup block
  must name `af`/`agent-admin` to remove them. Every such line ends with the
  `# fleet-legacy-cleanup` marker, and verification scans filter that marker.
- Three binaries: `fleet`, `fleet-admin`, `fleet-daemon`.
- `fleet` must not depend on the `fleet-admin` or `fleet-daemon` crates.
- Token prefixes: admin `fla_`, machine `flm_`, user `flu_`.
- Environment variables: `FLEET_*` only.
- Paths: `~/.fleet/...`; databases `fleet-admin.db`, `fleet-daemon.db`.
- Build with the prebuilt cargo binary:
  ```bash
  CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo <args>
  ```
- Every commit must end with:
  ```text
  Co-Authored-By: Claude Code <noreply@anthropic.com>
  ```
- Run `cargo fmt --all` and clippy before the final task.

## Rebrand boundary

Renamed in this plan (operational identifiers):

- binaries: `af` → `fleet`, `agent-admin` → `fleet-admin`;
- CLI namespaces and command names;
- environment variables: `AGENT_FLEET_*` → `FLEET_*`;
- paths: `~/.agent-fleet` → `~/.fleet`;
- databases: `agent-admin.db` → `fleet-admin.db`, `agent.db` → `fleet-daemon.db`;
- token prefixes: `af_admin_` → `fla_`, `afm_` → `flm_`, `afu_` → `flu_`;
- systemd units and service users;
- keyring service name `agent-fleet` → `fleet`;
- Cloudflare tunnel name `agent-fleet` → `fleet`;
- Cloudflare Access app name `Agent Fleet` → `Fleet`;
- the Git write header `X-Agent-Fleet-Git-Token` → `X-Fleet-Git-Token`.

Kept unchanged in this plan:

- the repository/product prose name "agent-fleet"/"Agent Fleet" in README
  titles, Cargo package names, npm package names (`@agent-fleet/*`), and
  historical design documents under `docs/superpowers/`;
- prose such as "agent admin" or "Agent Fleet" when it refers to the product
  rather than an operational identifier.

The verification scans below intentionally do not fail on the kept product
names. They do fail on operational leftovers, except lines marked
`# fleet-legacy-cleanup` in `scripts/install/install.sh` (the one-time removal
of the old binaries/units).

## File map

**Binaries and server crates**
- Modify: `apps/fleet-cli/Cargo.toml`
- Modify: `apps/fleet-cli/src/main.rs`
- Modify: `apps/fleet-cli/src/cli.rs`
- Modify: `apps/fleet-cli/src/config.rs`
- Modify: `apps/fleet-cli/src/secrets.rs`
- Modify: `apps/fleet-cli/src/pairing.rs`
- Modify: `apps/fleet-cli/src/network.rs`
- Modify: `apps/fleet-cli/src/connect.rs`
- Modify: `apps/fleet-admin/Cargo.toml`
- Modify: `apps/fleet-admin/src/main.rs`
- Create: `apps/fleet-admin/src/lib.rs`
- Modify: `apps/fleet-daemon/Cargo.toml`
- Modify: `apps/fleet-daemon/src/main.rs`
- Create: `apps/fleet-daemon/src/lib.rs`

**Core crates**
- Modify: `crates/core-auth/src/token.rs`
- Modify: `crates/core-auth/src/pairing.rs`
- Modify: `crates/core-auth/src/lib.rs`
- Modify: `crates/core-config/src/lib.rs`
- Modify: `crates/core-config/examples/daemon.toml`
- Rename: `crates/core-config/examples/agent-admin.toml` → `crates/core-config/examples/admin.toml`
- Modify: `crates/core-config/src/lib.rs` `include_str!("../examples/admin.toml")`

**Commands and packaging**
- Modify: `package.json`
- Modify: `scripts/install/install.sh`
- Rename: `packaging/systemd/agent-admin.service.in` → `packaging/systemd/fleet-admin.service.in`
- Rename: `packaging/systemd/agent-daemon.service.in` → `packaging/systemd/fleet-daemon.service.in`
- Modify: `scripts/test/install_check.sh`
- Modify: `scripts/test/docker_verify.sh`
- Modify: `scripts/test/git_write_e2e.sh`
- Modify: `scripts/test/multi_machine_stress.sh`
- Modify: `scripts/test/ui_e2e.sh`
- Modify: `scripts/test/e2e_smoke.sh`
- Modify: `.env.example` (hidden file; use `rg --hidden`)
- Modify: `README.md`
- Modify: `docs/development.md`, `docs/connectivity.md`, `docs/security.md`, `docs/crate-interfaces.md`, `docs/mvp-status.md`
- Modify: `docs/architecture.md`, `docs/implementation.md`, `docs/protocols.md`
- Modify: `apps/docs/src/content/docs/**`

---

### Task 1: Rename the CLI binary to `fleet`

**Files:**
- Modify: `apps/fleet-cli/Cargo.toml`
- Modify: `apps/fleet-cli/src/cli.rs`
- Modify: `package.json`

- [ ] **Step 1: Verify the old target name no longer builds**

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo build -p fleet-cli --bin fleet
```
Expected: FAIL with "no bin target named `fleet`".

- [ ] **Step 2: Rename the bin target**

Change `apps/fleet-cli/Cargo.toml`:

```toml
[[bin]]
name = "fleet"
path = "src/main.rs"
```

- [ ] **Step 3: Rename the clap command**

In `apps/fleet-cli/src/cli.rs`, change:

```rust
#[command(
    name = "af",
    version,
    about = "Agent Fleet command line: manage the admin API and remote access"
)]
pub struct Cli {
```

to:

```rust
#[command(
    name = "fleet",
    version,
    about = "Fleet command line: manage the admin, daemons, and remote access"
)]
pub struct Cli {
```

- [ ] **Step 4: Update package scripts**

In `package.json`, replace the `"af"` script with:

```json
"fleet": "${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo} run -q -p fleet-cli --bin fleet",
```

Update the `"help"` string to list `fleet` instead of `af`.

- [ ] **Step 5: Build and run help**

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo build -p fleet-cli --bin fleet
./target/debug/fleet --help | head -20
```
Expected: build succeeds and `Usage: fleet` appears.

- [ ] **Step 6: Commit**

```bash
git add apps/fleet-cli/Cargo.toml apps/fleet-cli/src/cli.rs package.json
git commit -m "refactor(cli): rename af binary and command to fleet" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: Convert `fleet-admin` to a library plus thin binary

**Files:**
- Modify: `apps/fleet-admin/Cargo.toml`
- Create: `apps/fleet-admin/src/lib.rs`
- Modify: `apps/fleet-admin/src/main.rs`

- [ ] **Step 1: Add the lib target placeholder test**

Create `apps/fleet-admin/src/lib.rs` with only this test first:

```rust
//! Fleet admin server library.

#[cfg(test)]
mod tests {
    #[test]
    fn lib_target_is_available() {
        assert_eq!(2 + 2, 4);
    }
}
```

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-admin --lib
```
Expected: PASS. Cargo auto-detects `src/lib.rs`, so this is a refactor
checkpoint rather than a red test. The behavior tests already cover `serve`.

- [ ] **Step 2: Add the library target and move the main logic**

Change `apps/fleet-admin/Cargo.toml`:

```toml
[[bin]]
name = "fleet-admin"
path = "src/main.rs"
```

Replace `apps/fleet-admin/src/lib.rs` with the current `main.rs` content, adapted:

```rust
//! Fleet admin server library.

mod app;
mod error;
mod middleware;
mod routes;
mod services;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use fleet_admin_storage::AdminStorage;
use fleet_core_config::AdminConfig;

use crate::services::event_service;
use crate::state::AppState;

/// Process-level CLI for `fleet-admin`.
#[derive(Debug, Parser)]
#[command(name = "fleet-admin", version, about)]
pub struct Cli {
    /// Path to a TOML config file (default: ~/.fleet/admin/config.toml).
    #[arg(long, global = true, env = "FLEET_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// `fleet-admin` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the fleet admin server (default).
    Serve(ServeArgs),
}

/// CLI overrides applied after the configuration file and environment.
#[derive(Debug, Default, Args)]
pub struct ServeArgs {
    /// Listener address, e.g. `127.0.0.1:8787`.
    #[arg(long)]
    pub bind: Option<String>,
    /// SQLite database path (`:memory:` is accepted for development).
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// Admin bearer token for the REST API and machine gateway.
    #[arg(long)]
    pub admin_token: Option<String>,
    /// Directory containing the built web UI.
    #[arg(long)]
    pub web_dir: Option<PathBuf>,
}

/// Run the admin process.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command.unwrap_or(Command::Serve(ServeArgs::default())) {
        Command::Serve(args) => serve(cli.config, args).await,
    }
}

async fn serve(config_path: Option<PathBuf>, args: ServeArgs) -> anyhow::Result<()> {
    // Keep the current body of `serve` from src/main.rs unchanged.
    // ...
}
```

Move `serve`, `parse_bind`, `resolve_address`, and `shutdown_signal` from
`src/main.rs` into `src/lib.rs` unchanged.

Replace `apps/fleet-admin/src/main.rs` with:

```rust
//! `fleet-admin` process entrypoint.

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fleet_core_common::init_tracing("fleet_admin=info");
    let cli = fleet_admin::Cli::parse();
    fleet_admin::run(cli).await
}
```

- [ ] **Step 3: Build and test**

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-admin --lib
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo build -p fleet-admin --bin fleet-admin
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-admin
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add apps/fleet-admin/src/lib.rs apps/fleet-admin/src/main.rs apps/fleet-admin/Cargo.toml
git commit -m "refactor(admin): split fleet-admin into lib and thin binary" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: Convert `fleet-daemon` to a library plus thin binary

**Files:**
- Modify: `apps/fleet-daemon/Cargo.toml`
- Create: `apps/fleet-daemon/src/lib.rs`
- Modify: `apps/fleet-daemon/src/main.rs`

- [ ] **Step 1: Add the lib target placeholder test**

Create `apps/fleet-daemon/src/lib.rs` with:

```rust
//! Fleet daemon server library.

#[cfg(test)]
mod tests {
    #[test]
    fn lib_target_is_available() {
        assert_eq!(2 + 2, 4);
    }
}
```

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-daemon --lib
```
Expected: PASS. Cargo auto-detects `src/lib.rs`; this is a refactor
checkpoint, not a red test.

- [ ] **Step 2: Add the library target and move the main logic**

Keep `apps/fleet-daemon/Cargo.toml` `[[bin]] name = "fleet-daemon"`.

Replace `apps/fleet-daemon/src/lib.rs` with the current `main.rs` content, adapted:

```rust
//! Fleet daemon server library.

mod app;
mod bootstrap;
mod delivery;
mod error;
mod events;
mod handler;
mod services;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Process-level CLI for `fleet-daemon`.
#[derive(Debug, Parser)]
#[command(name = "fleet-daemon", version, about)]
pub struct Cli {
    /// Path to a TOML config file (default: ~/.fleet/daemon/config.toml).
    #[arg(long, global = true, env = "FLEET_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// `fleet-daemon` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon (default).
    Serve,
    /// Register a local directory as a workspace and print it as JSON.
    RegisterWorkspace {
        /// Absolute path of the workspace root.
        root: PathBuf,
        /// Display name (defaults to the directory name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Print detected agent providers as JSON.
    Providers,
    /// Rotate the machine Ed25519 key. Stop the daemon first.
    RotateKey,
    /// Pair this machine with the fleet admin and save the per-machine token.
    Pair {
        /// Admin WebSocket URL, e.g. ws://127.0.0.1:8787/ws/machine
        #[arg(long)]
        server: String,
        /// Short-lived pairing code.
        #[arg(long)]
        code: String,
        /// Machine display name (defaults to the configured name).
        #[arg(long)]
        name: Option<String>,
    },
}

/// Run the daemon process.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => app::serve(cli.config).await,
        Command::RegisterWorkspace { root, name } => {
            bootstrap::register_workspace(cli.config, root, name).await
        }
        Command::Providers => bootstrap::print_providers(cli.config).await,
        Command::RotateKey => bootstrap::rotate_key(cli.config).await,
        Command::Pair { server, code, name } => {
            bootstrap::pair(cli.config, server, code, name).await
        }
    }
}
```

Replace `apps/fleet-daemon/src/main.rs` with:

```rust
//! `fleet-daemon` process entrypoint.

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fleet_core_common::init_tracing("fleet_daemon=info,fleet_daemon_runtime=info");
    let cli = fleet_daemon::Cli::parse();
    fleet_daemon::run(cli).await
}
```

- [ ] **Step 3: Build and test**

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-daemon --lib
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo build -p fleet-daemon --bin fleet-daemon
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-daemon
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add apps/fleet-daemon/src/lib.rs apps/fleet-daemon/src/main.rs apps/fleet-daemon/Cargo.toml
git commit -m "refactor(daemon): split fleet-daemon into lib and thin binary" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

**Checkpoint:** from Task 2 until Task 10, `package.json` `dev:admin` and the
shell/E2E scripts still reference `agent-admin`/`target/debug/agent-admin`.
`cargo test` is the source of truth during this interval. Do not run the
installer or E2E scripts until Task 10/11.

---

### Task 4: Rebrand token prefixes and add `generate_admin_token`

**Files:**
- Modify: `crates/core-auth/src/token.rs`
- Modify: `crates/core-auth/src/pairing.rs`
- Modify: `crates/core-auth/src/lib.rs`
- Test: inline `#[cfg(test)]` modules

- [ ] **Step 1: Write failing prefix tests**

In `crates/core-auth/src/token.rs` tests, change the user-token assertion to:

```rust
assert!(first.starts_with("flu_"));
```

In `crates/core-auth/src/pairing.rs` tests, add:

```rust
#[test]
fn generated_admin_and_machine_tokens_use_fleet_prefixes() {
    assert!(super::generate_machine_token().starts_with("flm_"));
    assert!(super::generate_admin_token().starts_with("fla_"));
}
```

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-auth
```
Expected: FAIL on the prefix assertions and missing `generate_admin_token`.

- [ ] **Step 2: Implement the new prefixes**

In `crates/core-auth/src/token.rs`:

```rust
/// Generate a new opaque user token like `flu_<64 hex chars>`.
pub fn generate_user_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("flu_{}", hex::encode(bytes))
}
```

In `crates/core-auth/src/pairing.rs`:

```rust
/// Generate an `fla_<64 hex>` admin token.
pub fn generate_admin_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("fla_{}", hex::encode(bytes))
}

/// Generate an `flm_<64 hex>` per-machine token.
pub fn generate_machine_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("flm_{}", hex::encode(bytes))
}
```

Update the export in `crates/core-auth/src/lib.rs`:

```rust
pub use pairing::{
    generate_admin_token, generate_machine_token, generate_pair_code, hash_pair_code,
    normalize_pair_code, PAIR_CODE_LEN,
};
```

- [ ] **Step 3: Run the tests**

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-auth
```
Expected: PASS.

- [ ] **Step 4: Update remaining old token prefixes**

Run a repository scan:

```bash
rg -n 'af_admin_|afm_|afu_' \
  --glob '!target/**' \
  --glob '!docs/superpowers/specs/**' \
  --glob '!docs/superpowers/plans/**'
```

Replace every non-historical occurrence with `fla_`, `flm_`, or `flu_` according to context. Check at least:

- `crates/core-auth/src/**`
- `apps/fleet-admin/src/**`
- `apps/fleet-daemon/src/**`
- `apps/fleet-cli/src/**`
- `scripts/**`
- `docs/**`
- `apps/docs/src/content/docs/**`
- `crates/core-config/examples/**`

Run the scan again and expect no output.

- [ ] **Step 5: Commit**

```bash
git add crates apps packages docs scripts
git commit -m "refactor(auth): rebrand token prefixes to fla/flm/flu" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: Rebrand environment variables to `FLEET_*`

**Files:**
- Modify: `crates/core-config/src/lib.rs`
- Modify: `crates/core-auth/src/token.rs`
- Modify: `apps/fleet-cli/src/cli.rs`
- Modify: `apps/fleet-cli/src/config.rs`
- Modify: `apps/fleet-cli/src/secrets.rs`
- Modify: `apps/fleet-admin/src/**` (main.rs/lib.rs and error hints)
- Modify: `apps/fleet-daemon/src/**` (error hints)
- Modify: `scripts/install/install.sh`
- Modify: `packaging/systemd/*.service.in`
- Modify: `.env.example`
- Test: inline env-name tests

- [ ] **Step 1: Add failing env-name tests**

In `crates/core-config/src/lib.rs` tests:

```rust
#[test]
fn env_vars_use_the_fleet_prefix() {
    assert_eq!(ENV_CONFIG, "FLEET_CONFIG");
    assert_eq!(ENV_ADMIN_TOKEN, "FLEET_ADMIN_TOKEN");
    assert_eq!(ENV_SERVER_URL, "FLEET_SERVER_URL");
    assert_eq!(ENV_PAIR_CODE, "FLEET_PAIR_CODE");
    assert_eq!(ENV_PAIR_CODE_TTL, "FLEET_PAIR_CODE_TTL");
    assert_eq!(ENV_DATA_DIR, "FLEET_DATA_DIR");
    assert_eq!(ENV_DB, "FLEET_DB");
    assert_eq!(ENV_BIND, "FLEET_BIND");
    assert_eq!(ENV_WEB_DIR, "FLEET_WEB_DIR");
    assert_eq!(ENV_GIT_WRITES, "FLEET_GIT_WRITES");
    assert_eq!(ENV_GIT_WRITE_TOKEN, "FLEET_GIT_WRITE_TOKEN");
    assert_eq!(ENV_MACHINE_NAME, "FLEET_MACHINE_NAME");
}
```

In `crates/core-auth/src/token.rs` tests:

```rust
#[test]
fn auth_env_vars_use_the_fleet_prefix() {
    assert_eq!(ADMIN_TOKEN_ENV, "FLEET_ADMIN_TOKEN");
    assert_eq!(DEV_MODE_ENV, "FLEET_DEV");
}
```

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-config
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-auth
```
Expected: FAIL on the assertions.

- [ ] **Step 2: Replace the constants**

In `crates/core-config/src/lib.rs`, change every `pub const ENV_*: &str = "AGENT_FLEET_...";` to `"FLEET_..."`.

In `crates/core-auth/src/token.rs`:

```rust
pub const ADMIN_TOKEN_ENV: &str = "FLEET_ADMIN_TOKEN";
pub const DEV_MODE_ENV: &str = "FLEET_DEV";
```

Update doc comments that mention `AGENT_FLEET_*`.

- [ ] **Step 3: Update CLI env attributes and messages**

In `apps/fleet-cli/src/cli.rs`:

```rust
#[arg(long, global = true, env = "FLEET_ADMIN_URL")]
pub admin_url: Option<String>,

#[arg(long, global = true, env = "FLEET_ADMIN_TOKEN")]
pub admin_token: Option<String>,

#[arg(long, global = true, env = "FLEET_CLI_CONFIG")]
pub cli_config: Option<PathBuf>,
```

Update error messages to mention `FLEET_ADMIN_TOKEN` and `fleet config init`.

In `apps/fleet-cli/src/config.rs`:

```rust
const CLI_CONFIG_ENV: &str = "FLEET_CLI_CONFIG";
```

In `apps/fleet-cli/src/secrets.rs`:

```rust
if let Ok(path) = std::env::var("FLEET_SECRETS_FILE") {
```

- [ ] **Step 4: Update scripts, packaging, and hidden env files**

Run with `--hidden` so `.env.example` is included:

```bash
rg -l --hidden 'AGENT_FLEET_' \
  --glob '!target/**' \
  --glob '!docs/superpowers/specs/**' \
  --glob '!docs/superpowers/plans/**'
```

Replace `AGENT_FLEET_` with `FLEET_` in every listed file. This includes
`scripts/install/install.sh`, `packaging/systemd/*.service.in`, `.env.example`,
tests, and docs.

Update `.env.example` explicitly:

```dotenv
# Fleet admin
FLEET_ADMIN_TOKEN=dev-token
FLEET_BIND=127.0.0.1:8787
FLEET_DB=~/.fleet/admin/fleet-admin.db

# Fleet daemon
FLEET_SERVER_URL=ws://127.0.0.1:8787/ws/machine
# Pair once with `fleet daemon setup` (or set FLEET_PAIR_CODE for automated
# provisioning). The per-machine token is stored in <key_file>.token.
FLEET_DATA_DIR=~/.fleet
FLEET_MACHINE_NAME=dev-machine
```

- [ ] **Step 4b: Replace constructed `.agent-fleet` paths**

The `~/.agent-fleet` pattern does not match `home_dir().join(".agent-fleet")`.
Replace every such site explicitly:

- `apps/fleet-cli/src/secrets.rs` keyring fallback path;
- `apps/fleet-cli/src/network.rs` Cloudflare config default;
- `apps/fleet-cli/src/cli.rs` Cloudflare config default;
- `scripts/install/install.sh` daemon key/state fallbacks.

Use:

```bash
rg -n --hidden '\.agent-fleet' \
  --glob '!target/**' \
  --glob '!docs/superpowers/**'
```

At Task 5 this scan still lists sites outside Task 5's file set:
`apps/fleet-cli/src/config.rs`, `crates/core-config/examples/*.toml`,
`docs/*.md`, and `README.md`. Record them; Task 6 handles the config/example
paths and Task 12 handles the docs. Do not require zero output until Task 13.

Verify no non-historical references remain:

```bash
rg -n --hidden 'AGENT_FLEET_' \
  --glob '!target/**' \
  --glob '!docs/superpowers/specs/**' \
  --glob '!docs/superpowers/plans/**'
```
Expected: no output.

- [ ] **Step 5: Run tests**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-config
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-auth
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates apps/fleet-cli/src apps/fleet-admin/src apps/fleet-daemon/src \
  apps/docs scripts/install/install.sh packaging/systemd .env.example \
  README.md package.json docs
git commit -m "refactor(config): rename environment variables to FLEET_*" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: Rebrand default paths and database names

**Files:**
- Modify: `crates/core-config/src/lib.rs`
- Modify: `apps/fleet-cli/src/config.rs`
- Modify: `apps/fleet-cli/src/network.rs`
- Modify: `apps/fleet-cli/src/connect.rs`
- Modify: `crates/core-config/examples/*.toml`
- Test: inline default-path tests

- [ ] **Step 1: Add failing default-path tests**

In `crates/core-config/src/lib.rs` tests:

```rust
#[test]
fn default_paths_use_the_fleet_directory_and_split_databases() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _guard = EnvGuard::clear();
    let home = dirs::home_dir().expect("home");
    assert_eq!(
        default_admin_config_path(),
        home.join(".fleet/admin/config.toml")
    );
    assert_eq!(
        default_daemon_config_path(),
        home.join(".fleet/daemon/config.toml")
    );
    assert_eq!(
        PathBuf::from(DEFAULT_ADMIN_DB),
        PathBuf::from("~/.fleet/admin/fleet-admin.db")
    );
    assert_eq!(
        PathBuf::from(DEFAULT_DAEMON_DB),
        PathBuf::from("~/.fleet/daemon/fleet-daemon.db")
    );
    assert_eq!(
        PathBuf::from(DEFAULT_KEY_FILE),
        PathBuf::from("~/.fleet/daemon/machine.key")
    );
}
```

Make `DEFAULT_ADMIN_DB`, `DEFAULT_DAEMON_DB`, `DEFAULT_KEY_FILE`,
`default_admin_config_path`, and `default_daemon_config_path` visible to the
test (keep the constants private if the test lives in the same module).

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-config
```
Expected: FAIL on paths/functions.

- [ ] **Step 2: Update constants and role-specific loaders**

In `crates/core-config/src/lib.rs`:

```rust
const DEFAULT_KEY_FILE: &str = "~/.fleet/daemon/machine.key";
const DEFAULT_DAEMON_DB: &str = "~/.fleet/daemon/fleet-daemon.db";
const DEFAULT_ADMIN_DB: &str = "~/.fleet/admin/fleet-admin.db";

/// Default admin config path, unless `FLEET_CONFIG` overrides it.
pub fn default_admin_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os(ENV_CONFIG) {
        return PathBuf::from(path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".fleet/admin/config.toml")
}

/// Default daemon config path, unless `FLEET_CONFIG` overrides it.
pub fn default_daemon_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os(ENV_CONFIG) {
        return PathBuf::from(path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".fleet/daemon/config.toml")
}
```

Update `AdminConfig::load_default()`:

```rust
pub fn load_default() -> anyhow::Result<Self> {
    let path = default_admin_config_path();
    Self::load(Some(&path))
}
```

Update `DaemonConfig::load_default()`:

```rust
pub fn load_default() -> anyhow::Result<Self> {
    let path = default_daemon_config_path();
    Self::load(Some(&path))
}
```

Remove the old shared `default_config_path()` helper once no callers remain.

- [ ] **Step 2b: Update `FLEET_DATA_DIR` derivation and existing defaults tests**

In `DaemonConfig::apply_env`:

```rust
if let Ok(data_dir) = std::env::var(ENV_DATA_DIR) {
    let data_dir = PathBuf::from(data_dir);
    self.storage.database = data_dir.join("daemon/fleet-daemon.db");
    self.machine.key_file = data_dir.join("daemon/machine.key");
}
```

In `AdminConfig::apply_env`:

```rust
if let Ok(data_dir) = std::env::var(ENV_DATA_DIR) {
    self.storage.database = PathBuf::from(data_dir).join("admin/fleet-admin.db");
}
```

Update the existing assertions that still expect `~/.agent-fleet/...`:

- `daemon_defaults_match_contract`
- `admin_defaults_match_contract`
- `env_overrides_toml_and_defaults`

New expectations:

```rust
assert_eq!(
    PathBuf::from("~/.fleet/daemon/machine.key"),
    PathBuf::from(DEFAULT_KEY_FILE)
);
assert_eq!(
    PathBuf::from("~/.fleet/daemon/fleet-daemon.db"),
    PathBuf::from(DEFAULT_DAEMON_DB)
);
assert_eq!(
    PathBuf::from("~/.fleet/admin/fleet-admin.db"),
    PathBuf::from(DEFAULT_ADMIN_DB)
);
```

For `FLEET_DATA_DIR` assertions, expect:

```rust
data_dir.join("daemon/fleet-daemon.db")
data_dir.join("daemon/machine.key")
data_dir.join("admin/fleet-admin.db")
```

- [ ] **Step 3: Update CLI config paths and network defaults**

In `apps/fleet-cli/src/config.rs`:

```rust
pub fn default_cli_config_path() -> PathBuf {
    if let Ok(path) = std::env::var(CLI_CONFIG_ENV) {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    home_dir().join(".fleet").join("cli.toml")
}

pub fn default_admin_config_path() -> PathBuf {
    fleet_core_config::default_admin_config_path()
}

pub fn default_daemon_config_path() -> PathBuf {
    fleet_core_config::default_daemon_config_path()
}
```

Remove `default_service_config_path()` after updating callers.

In `apps/fleet-cli/src/network.rs`, change `WizardOptions::new`:

```rust
admin_config: default_admin_config_path(),
daemon_config: default_daemon_config_path(),
```

In `apps/fleet-cli/src/cli.rs`, update `network_command` and `connect`
defaults:

```rust
options.admin_config = admin_config.unwrap_or_else(default_admin_config_path);
options.daemon_config = daemon_config.unwrap_or_else(default_daemon_config_path);
```

and:

```rust
let admin_config = admin_config.unwrap_or_else(default_admin_config_path);
let daemon_config = daemon_config.unwrap_or_else(default_daemon_config_path);
```

Update every `default_service_config_path` caller:

- `apps/fleet-cli/src/network.rs`: import and `WizardOptions::new` defaults,
  plus the `cloudflared_config` default under `~/.fleet/`.
- `apps/fleet-cli/src/cli.rs`: `network_command`, the `connect` dispatch, and
  the `cloudflared_config` defaults.
- `apps/fleet-cli/src/connect.rs`: the test helper and any default paths.
- `apps/fleet-cli/src/config.rs`: delete the helper after all callers move.

Run:

```bash
rg -n 'default_service_config_path' apps/fleet-cli
```
Expected after the change: no output.

- [ ] **Step 4: Update examples and rename the admin example**

```bash
git mv crates/core-config/examples/agent-admin.toml \
  crates/core-config/examples/admin.toml
```

In `crates/core-config/examples/admin.toml`, use explicit role paths:

```toml
[storage]
database = "~/.fleet/admin/fleet-admin.db"

[web]
static_dir = "/usr/local/share/fleet/web"
```

In `crates/core-config/examples/daemon.toml`:

```toml
[machine]
key_file = "~/.fleet/daemon/machine.key"

[storage]
database = "~/.fleet/daemon/fleet-daemon.db"
```

Update the `include_str!` in `crates/core-config/src/lib.rs`:

```rust
let admin: AdminConfig = toml::from_str(include_str!("../examples/admin.toml"))
```

- [ ] **Step 5: Run tests**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-core-config
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/core-config apps/fleet-cli
git commit -m "refactor(config): split admin/daemon paths and databases under ~/.fleet" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 7: Add `admin_config` and `daemon_config` to the CLI config

**Files:**
- Modify: `apps/fleet-cli/src/config.rs`
- Modify: `apps/fleet-cli/src/cli.rs`
- Test: `apps/fleet-cli/src/config.rs`

- [ ] **Step 1: Write a failing round-trip test**

Replace the existing `cli_config_round_trip` test with:

```rust
#[test]
fn cli_config_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cli.toml");
    let config = CliConfig {
        admin_url: Some(DEFAULT_ADMIN_URL.into()),
        token: Some("fla_test".into()),
        output: Some("json".into()),
        public_url: Some("https://fleet.example.com".into()),
        admin_config: Some(dir.path().join("admin.toml")),
        daemon_config: Some(dir.path().join("daemon.toml")),
    };
    config.save(&path).unwrap();
    let loaded = CliConfig::load(&path).unwrap();
    assert_eq!(loaded.admin_url.as_deref(), Some(DEFAULT_ADMIN_URL));
    assert_eq!(loaded.token.as_deref(), Some("fla_test"));
    assert_eq!(loaded.output.as_deref(), Some("json"));
    assert_eq!(loaded.public_url.as_deref(), Some("https://fleet.example.com"));
    assert_eq!(loaded.admin_config, config.admin_config);
    assert_eq!(loaded.daemon_config, config.daemon_config);
}
```

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli cli_config_round_trip
```
Expected: FAIL because the fields do not exist.

- [ ] **Step 2: Add the fields**

In `apps/fleet-cli/src/config.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CliConfig {
    pub admin_url: Option<String>,
    pub token: Option<String>,
    pub output: Option<String>,
    pub public_url: Option<String>,
    pub admin_config: Option<PathBuf>,
    pub daemon_config: Option<PathBuf>,
}
```

Update the `config_command` struct literal in `apps/fleet-cli/src/cli.rs`:

```rust
let config = CliConfig {
    admin_url: Some(url.unwrap_or_else(|| ctx.admin_url.clone())),
    token: token.or_else(|| ctx.token.clone()),
    output: format.or(Some(if ctx.json { "json" } else { "table" }.to_string())),
    public_url: ctx.config.public_url.clone(),
    admin_config: ctx
        .config
        .admin_config
        .clone()
        .or_else(|| Some(default_admin_config_path())),
    daemon_config: ctx
        .config
        .daemon_config
        .clone()
        .or_else(|| Some(default_daemon_config_path())),
};
```

Import `default_admin_config_path` / `default_daemon_config_path` from
`crate::config`.

- [ ] **Step 3: Use the recorded paths in network/connect defaults**

In `apps/fleet-cli/src/cli.rs`, `network_command`:

```rust
options.admin_config = admin_config
    .or_else(|| ctx.config.admin_config.clone())
    .unwrap_or_else(default_admin_config_path);
options.daemon_config = daemon_config
    .or_else(|| ctx.config.daemon_config.clone())
    .unwrap_or_else(default_daemon_config_path);
```

In the `connect` dispatch, use the same `.or_else(|| ctx.config... )` pattern.

- [ ] **Step 4: Run tests**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/fleet-cli/src/config.rs apps/fleet-cli/src/cli.rs
git commit -m "feat(cli): record admin and daemon config paths in cli.toml" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 8: Introduce the nested command tree

**Files:**
- Modify: `apps/fleet-cli/src/cli.rs`
- Test: inline `#[cfg(test)]` module in `cli.rs`

- [ ] **Step 1: Write failing parse tests**

Add at the bottom of `apps/fleet-cli/src/cli.rs`:

```rust
#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn parses_admin_machines() {
        let cli = Cli::try_parse_from(["fleet", "admin", "machines", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Admin {
                action: AdminCommand::Machines {
                    action: MachinesCommand::List
                }
            }
        ));
    }

    #[test]
    fn parses_daemon_setup() {
        let cli = Cli::try_parse_from([
            "fleet",
            "daemon",
            "setup",
            "--server",
            "ws://127.0.0.1:8787/ws/machine",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Daemon {
                action: DaemonCommand::Setup { .. }
            }
        ));
    }

    #[test]
    fn parses_top_level_config_and_network() {
        assert!(matches!(
            Cli::try_parse_from(["fleet", "config", "path"]).unwrap().command,
            Command::Config {
                action: ConfigCommand::Path
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["fleet", "network", "detect"])
                .unwrap()
                .command,
            Command::Network {
                action: NetworkCommand::Detect
            }
        ));
    }

    #[test]
    fn parses_setup_status_and_doctor() {
        assert!(matches!(
            Cli::try_parse_from(["fleet", "setup"]).unwrap().command,
            Command::Setup
        ));
        assert!(matches!(
            Cli::try_parse_from(["fleet", "status"]).unwrap().command,
            Command::Status
        ));
        assert!(matches!(
            Cli::try_parse_from(["fleet", "doctor"]).unwrap().command,
            Command::Doctor
        ));
    }
}
```

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli cli_tests
```
Expected: FAIL; the new command variants do not exist.

- [ ] **Step 1b: Update the existing CLI tests to the new flags**

The existing `#[cfg(test)] mod tests` in `cli.rs` uses the old flags and
command variants. Update:

- `cli.url` → `cli.admin_url`;
- `cli.token` → `cli.admin_token` where present;
- `parse(&["af", ...])` → `parse(&["fleet", ...])`;
- `--url` → `--admin-url`;
- `Command::Machines { ... }` → `Command::Admin { action: AdminCommand::Machines { ... } }`;
- `Command::Users { ... }` → `Command::Admin { action: AdminCommand::Users { ... } }`;
- `Command::Network { ... }` stays top-level;
- `rejects_unknown_roles` becomes
  `parse(&["fleet", "admin", "users", "create", ...])`;
- `config_init_force_backs_up_existing_config` (around `cli.rs:1265`) changes
  `ConfigCommand::Init { url, token, .. }` to
  `ConfigCommand::Init { admin_url, admin_token, .. }`.

Add a temporary parser test for the old namespace absence:

```rust
#[test]
fn old_top_level_machine_command_is_rejected() {
    assert!(Cli::try_parse_from(["fleet", "machines", "list"]).is_err());
}
```

Run:
```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli
```
Expected: FAIL until Step 2/3 are applied. This keeps the existing test suite
aligned with the rename.

- [ ] **Step 2: Rename global flags and add the top-level command enum**

Update the clap import at the top of `apps/fleet-cli/src/cli.rs`:

```rust
use clap::{Args, Parser, Subcommand, ValueEnum};
```

In `apps/fleet-cli/src/cli.rs`, replace the `Cli` fields:

```rust
pub struct Cli {
    /// Agent admin base URL.
    #[arg(long, global = true, env = "FLEET_ADMIN_URL")]
    pub admin_url: Option<String>,

    /// Admin bearer token.
    #[arg(long, global = true, env = "FLEET_ADMIN_TOKEN")]
    pub admin_token: Option<String>,

    /// CLI configuration file (default: ~/.fleet/cli.toml).
    #[arg(long, global = true, env = "FLEET_CLI_CONFIG")]
    pub cli_config: Option<PathBuf>,

    /// Print raw JSON instead of tables.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}
```

Replace `Command` with:

```rust
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect or create the CLI configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Admin control-plane commands.
    Admin {
        #[command(subcommand)]
        action: AdminCommand,
    },
    /// Daemon/worker commands.
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },
    /// Detect and configure Tailscale / Cloudflare remote access.
    Network {
        #[command(subcommand)]
        action: NetworkCommand,
    },
    /// Interactive setup dispatcher.
    Setup,
    /// Summarize admin and daemon status.
    Status,
    /// Diagnose configuration and process state.
    Doctor,
}
```

- [ ] **Step 3: Add `AdminCommand` and `DaemonCommand`**

Add:

```rust
#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Set up the admin service.
    Setup,
    /// Inspect or validate the admin service configuration.
    Config {
        #[command(subcommand)]
        action: AdminConfigCommand,
    },
    /// Start the admin service.
    Start(ProcessArgs),
    /// Stop the admin service.
    Stop(ProcessArgs),
    /// Restart the admin service.
    Restart(ProcessArgs),
    /// Show admin process and health status.
    Status(ProcessArgs),
    /// Show admin logs.
    Logs(LogArgs),
    /// Run the admin in the foreground.
    Run,
    /// Install the systemd unit.
    InstallService(ServiceArgs),
    /// Remove the systemd unit.
    UninstallService(ServiceArgs),
    /// Check that the admin is reachable.
    Health,
    /// Manage machines.
    Machines {
        #[command(subcommand)]
        action: MachinesCommand,
    },
    /// Manage workspaces.
    Workspaces {
        #[command(subcommand)]
        action: WorkspacesCommand,
    },
    /// Manage agent sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionsCommand,
    },
    /// Manage users and tokens.
    Users {
        #[command(subcommand)]
        action: UsersCommand,
    },
    /// Manage organizations.
    Orgs {
        #[command(subcommand)]
        action: OrgsCommand,
    },
    /// Inspect the audit trail.
    Audit {
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Approve or reject a pending permission.
    Permissions {
        #[command(subcommand)]
        action: PermissionsCommand,
    },
}

#[allow(dead_code)] // fields are read by Plan 2 `config show`
#[derive(Debug, Subcommand)]
pub enum AdminConfigCommand {
    Show {
        #[arg(long)]
        reveal: bool,
    },
    Path,
    Validate,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Set up and pair this machine as a daemon.
    Setup {
        /// Organization id (default: org_default).
        #[arg(long)]
        org: Option<String>,
        /// Admin WebSocket URL (default: derived from --admin-url).
        #[arg(long)]
        server: Option<String>,
        /// Daemon TOML to update.
        #[arg(long, default_value = "~/.fleet/daemon/config.toml")]
        daemon_config: String,
        /// Machine display name.
        #[arg(long)]
        name: Option<String>,
        /// Save the admin token to the CLI configuration.
        #[arg(long)]
        store_token: bool,
        /// Allow plaintext ws:// on a non-loopback host (trusted LAN only).
        #[arg(long)]
        allow_insecure: bool,
        /// Only write configuration; do not create a code or pair.
        #[arg(long)]
        configure_only: bool,
    },
    /// Inspect or validate the daemon service configuration.
    Config {
        #[command(subcommand)]
        action: DaemonConfigCommand,
    },
    Start(ProcessArgs),
    Stop(ProcessArgs),
    Restart(ProcessArgs),
    Status(ProcessArgs),
    Logs(LogArgs),
    Run,
    InstallService(ServiceArgs),
    UninstallService(ServiceArgs),
    /// Redeem a pairing code without rewriting configuration.
    Pair {
        #[arg(long)]
        server: String,
        #[arg(long)]
        code: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Print detected agent providers.
    Providers,
    /// Manage local workspaces.
    Workspace {
        #[command(subcommand)]
        action: DaemonWorkspaceCommand,
    },
    /// Rotate the machine Ed25519 key.
    RotateKey,
    /// Show machine identity.
    Identity {
        #[command(subcommand)]
        action: IdentityCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum DaemonConfigCommand {
    Show,
    Path,
    Validate,
}

#[allow(dead_code)] // fields are read by Plan 2 `daemon workspace`
#[derive(Debug, Subcommand)]
pub enum DaemonWorkspaceCommand {
    Add {
        root: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    List,
    Remove {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum IdentityCommand {
    Show,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum ProcessMode {
    #[default]
    Auto,
    Systemd,
    Supervisor,
}

#[derive(Debug, Clone, Args)]
pub struct ProcessArgs {
    #[arg(long, value_enum, default_value = "auto")]
    pub mode: ProcessMode,
    #[arg(long, default_value_t = 15)]
    pub timeout: u64,
}

#[derive(Debug, Clone, Args)]
pub struct LogArgs {
    #[arg(short = 'f', long)]
    pub follow: bool,
    #[arg(short = 'n', long, default_value_t = 200)]
    pub lines: usize,
    #[arg(long, value_enum, default_value = "auto")]
    pub mode: ProcessMode,
}

#[derive(Debug, Clone, Args)]
pub struct ServiceArgs {
    #[arg(long)]
    pub system: bool,
    #[arg(long)]
    pub user: bool,
}
```

- [ ] **Step 3b: Rename `fleet config init` flags**

In `ConfigCommand::Init`, rename the fields:

```rust
Init {
    #[arg(long)]
    admin_url: Option<String>,
    #[arg(long)]
    admin_token: Option<String>,
    #[arg(long, value_parser = ["table", "json"])]
    output: Option<String>,
    #[arg(long)]
    force: bool,
},
```

Update `config_command`:

```rust
ConfigCommand::Init {
    admin_url,
    admin_token,
    output: format,
    force,
} => {
    // ...
    let config = CliConfig {
        admin_url: Some(admin_url.unwrap_or_else(|| ctx.admin_url.clone())),
        token: admin_token.or_else(|| ctx.token.clone()),
        // ...
    };
}
```

**Deferred to Plan 2:** the namespace-level `--config`, `--state-dir`,
`--bin`, and `--system|--user` flags on `fleet admin` / `fleet daemon`. Plan 1
puts `--system`/`--user` only on `ServiceArgs`; Plan 2 moves them to the
namespace when setup/config commands need them.

- [ ] **Step 4: Add temporary dispatch for new commands**

In `run()`, replace the command match with:

```rust
match cli.command {
    Command::Config { action } => config_command(&ctx, action).await,
    Command::Admin { action } => admin_command(&ctx, action).await,
    Command::Daemon { action } => daemon_command(&ctx, action).await,
    Command::Network { action } => network_command(&ctx, action).await,
    Command::Setup => {
        bail!("`fleet setup` is implemented in Plan 2; use `fleet admin setup` or `fleet daemon setup`")
    }
    Command::Status => {
        bail!("`fleet status` is implemented in Plan 4")
    }
    Command::Doctor => {
        bail!("`fleet doctor` is implemented in Plan 4")
    }
}
```

Add dispatch helpers:

```rust
async fn admin_command(ctx: &AppContext, action: AdminCommand) -> Result<()> {
    match action {
        AdminCommand::Health => health(ctx).await,
        AdminCommand::Setup => {
            bail!("`fleet admin setup` is implemented in Plan 2")
        }
        AdminCommand::Config { action } => {
            let _ = action;
            bail!("`fleet admin config` is implemented in Plan 2")
        }
        AdminCommand::Start(args)
        | AdminCommand::Stop(args)
        | AdminCommand::Restart(args)
        | AdminCommand::Status(args) => {
            let _ = (args.mode, args.timeout);
            bail!("admin process lifecycle is implemented in Plan 3")
        }
        AdminCommand::Logs(args) => {
            let _ = (args.mode, args.lines, args.follow);
            bail!("admin logs are implemented in Plan 3")
        }
        AdminCommand::Run => {
            bail!("`fleet admin run` is implemented in Plan 3")
        }
        AdminCommand::InstallService(args)
        | AdminCommand::UninstallService(args) => {
            let _ = (args.system, args.user);
            bail!("admin service installation is implemented in Plan 2")
        }
        AdminCommand::Machines { action } => machines(ctx, action).await,
        AdminCommand::Workspaces { action } => workspaces(ctx, action).await,
        AdminCommand::Sessions { action } => sessions(ctx, action).await,
        AdminCommand::Users { action } => users(ctx, action).await,
        AdminCommand::Orgs { action } => orgs(ctx, action).await,
        AdminCommand::Audit { limit } => audit(ctx, limit).await,
        AdminCommand::Permissions { action } => permissions(ctx, action).await,
    }
}

async fn daemon_command(ctx: &AppContext, action: DaemonCommand) -> Result<()> {
    match action {
        DaemonCommand::Setup {
            org,
            server,
            daemon_config,
            name,
            store_token,
            allow_insecure,
            configure_only,
        } => {
            let daemon_config =
                crate::config::expand_tilde(std::path::PathBuf::from(daemon_config));
            crate::pairing::setup(crate::pairing::SetupOptions {
                admin_url: ctx.admin_url.clone(),
                token: ctx.token.clone(),
                org_id: org,
                server_url: server,
                daemon_config,
                name,
                store_token,
                allow_insecure,
                configure_only,
                cli_config: Some(ctx.config_path.clone()),
            })
            .await
        }
        DaemonCommand::Config { action } => {
            let _ = action;
            bail!("`fleet daemon config` is implemented in Plan 2")
        }
        DaemonCommand::Start(args)
        | DaemonCommand::Stop(args)
        | DaemonCommand::Restart(args)
        | DaemonCommand::Status(args) => {
            let _ = (args.mode, args.timeout);
            bail!("daemon process lifecycle is implemented in Plan 3")
        }
        DaemonCommand::Logs(args) => {
            let _ = (args.mode, args.lines, args.follow);
            bail!("daemon logs are implemented in Plan 3")
        }
        DaemonCommand::Run => {
            bail!("`fleet daemon run` is implemented in Plan 3")
        }
        DaemonCommand::InstallService(args)
        | DaemonCommand::UninstallService(args) => {
            let _ = (args.system, args.user);
            bail!("daemon service installation is implemented in Plan 2")
        }
        DaemonCommand::Pair { server, code, name } => {
            let _ = (server, code, name);
            bail!("`fleet daemon pair` is implemented in Plan 2")
        }
        DaemonCommand::Providers => {
            bail!("`fleet daemon providers` is implemented in Plan 2")
        }
        DaemonCommand::Workspace { action } => {
            let _ = action;
            bail!("`fleet daemon workspace` is implemented in Plan 2")
        }
        DaemonCommand::RotateKey => {
            bail!("`fleet daemon rotate-key` is implemented in Plan 2")
        }
        DaemonCommand::Identity { action } => {
            let _ = action;
            bail!("`fleet daemon identity` is implemented in Plan 2")
        }
    }
}
```

**Known deviation:** Plan 1 still constructs `AdminClient` eagerly in `run()`.
The `AppContext` split from spec §16.3 is a Plan 2 task. `AdminClient::new`
only validates the URL, so local commands still parse and run; Plan 2 removes
the eager construction when it adds the admin/daemon config commands.

Every placeholder branch above binds and discards its fields (`let _ = ...`)
so clippy's `dead_code` lint stays clean until the later plan reads them.

Update `run()` to use the renamed `Cli` fields:

```rust
let cli = Cli::parse();
let config_path = cli.cli_config.clone().unwrap_or_else(default_cli_config_path);
let config = CliConfig::load(&config_path)?;
let admin_url = cli
    .admin_url
    .clone()
    .or_else(|| config.admin_url.clone())
    .unwrap_or_else(|| DEFAULT_ADMIN_URL.to_string());
let token = cli
    .admin_token
    .clone()
    .or_else(|| config.token.clone());
```

Update all internal references from `cli.url`/`cli.token`/`cli.config` to the
new names.

- [ ] **Step 5: Remove the old top-level `Setup` and admin command variants**

Delete the old `Command::Setup { ... }`, `Command::Health`,
`Command::Machines`, `Command::Workspaces`, `Command::Sessions`,
`Command::Users`, `Command::Orgs`, `Command::Audit`, and
`Command::Permissions` variants. Their data types are reused under
`AdminCommand`.

- [ ] **Step 6: Run parse tests**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli cli_tests
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo build -p fleet-cli --bin fleet
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/fleet-cli/src/cli.rs
git commit -m "feat(cli): split commands into fleet admin and fleet daemon namespaces" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 9: Update help text and error hints for the new names

**Files:**
- Modify: `apps/fleet-cli/src/cli.rs`
- Modify: `apps/fleet-cli/src/pairing.rs`
- Modify: `apps/fleet-admin/src/lib.rs`
- Modify: `apps/fleet-admin/src/**`
- Modify: `apps/fleet-daemon/src/lib.rs`
- Modify: `apps/fleet-daemon/src/**`

- [ ] **Step 1: Scan for stale user-facing text**

Run with hidden files included:

```bash
rg -n --hidden '\baf\b|agent-admin|~/.agent-fleet|AGENT_FLEET_|\.agent-fleet' \
  apps crates scripts packaging docs README.md .env.example \
  --glob '!docs/superpowers/**'
```

- [ ] **Step 2: Replace user-facing references**

Examples:

- `` `af config init` `` → `` `fleet config init` ``
- `` `af setup` `` → `` `fleet daemon setup` ``
- `agent-admin` → `fleet-admin`
- `AGENT_FLEET_ADMIN_TOKEN` → `FLEET_ADMIN_TOKEN`
- `~/.agent-fleet/config.toml` → the role-specific path

Do not change historical specs/plans.

- [ ] **Step 3: Verify help output**

Run:
```bash
./target/debug/fleet --help
./target/debug/fleet admin --help
./target/debug/fleet daemon --help
./target/debug/fleet config --help
./target/debug/fleet network --help
```
Expected: commands appear under the intended namespaces.

- [ ] **Step 4: Commit**

```bash
git add apps/fleet-cli/src apps/fleet-admin/src apps/fleet-daemon/src crates
git commit -m "docs(cli): update help and error hints for fleet names" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 9b: Rebrand remaining operational identifiers

**Files:**
- Modify: `apps/fleet-cli/src/secrets.rs`
- Modify: `apps/fleet-cli/src/network.rs`
- Modify: `apps/fleet-cli/src/connect.rs`
- Modify: `apps/fleet-cli/src/cloudflare.rs`
- Modify: `apps/fleet-admin/src/routes/git.rs`
- Modify: `apps/fleet-admin/src/app.rs`
- Modify: `packages/api-client/src/api-client.ts`
- Modify: Rust/TypeScript/tests/docs containing
  `X-Agent-Fleet-Git-Token` (case-insensitive)

- [ ] **Step 1: Rename the keyring service**

In `apps/fleet-cli/src/secrets.rs`:

```rust
const SERVICE: &str = "fleet";
```

- [ ] **Step 2: Rename the Cloudflare tunnel and Access app**

In `apps/fleet-cli/src/network.rs` and `apps/fleet-cli/src/connect.rs`, replace
the tunnel name `"agent-fleet"` with `"fleet"` (tunnel creation, lookup, and
generated config comments).

In `apps/fleet-cli/src/cloudflare.rs`, replace the Access app display name
`"Agent Fleet"` with `"Fleet"`.

- [ ] **Step 3: Rename the Git write header**

Replace every occurrence of:

```text
X-Agent-Fleet-Git-Token
```

with:

```text
X-Fleet-Git-Token
```

Check Rust, TypeScript, browser code, tests, shell scripts, and docs. Known
sites:

- `packages/api-client/src/api-client.ts` (client sends the header);
- `apps/fleet-admin/src/routes/git.rs` (server reads the header);
- `apps/fleet-admin/src/app.rs` (CORS allowed-header list);
- `scripts/test/git_write_e2e.sh` and other tests;
- docs that show the header.

- [ ] **Step 4: Verify**

Use a case-insensitive scan, because the server constant is lowercase:

```bash
rg -ni --hidden 'x-agent-fleet-git-token' \
  apps crates packages scripts docs README.md \
  --glob '!docs/superpowers/**' \
  --glob '!target/**'
```

Expected: no output.

Then verify the other operational constants:

```bash
rg -n --hidden 'SERVICE = "agent-fleet"|"agent-fleet"|"Agent Fleet"' \
  apps crates packages scripts docs README.md \
  --glob '!docs/superpowers/**' \
  --glob '!target/**'
```

Expected: only product-prose/npm/package-name hits that the rebrand boundary
allows. Rename any tunnel/Access/keyring hit.

- [ ] **Step 5: Run tests**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-cli
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test -p fleet-admin
pnpm run test:web
```
Expected: PASS after the TypeScript header rename.

- [ ] **Step 6: Commit**

```bash
git add apps crates packages scripts docs
git commit -m "refactor: rebrand keyring, tunnel, access app, and git header" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 10: Rename packaging, installer, and package scripts

**Files:**
- Modify: `package.json`
- Modify: `scripts/install/install.sh`
- Rename: `packaging/systemd/agent-admin.service.in` → `packaging/systemd/fleet-admin.service.in`
- Rename: `packaging/systemd/agent-daemon.service.in` → `packaging/systemd/fleet-daemon.service.in`

- [ ] **Step 1: Rename the unit templates**

```bash
git mv packaging/systemd/agent-admin.service.in packaging/systemd/fleet-admin.service.in
git mv packaging/systemd/agent-daemon.service.in packaging/systemd/fleet-daemon.service.in
```

Update both files. **Plan 1 keeps direct server binaries** because
`fleet admin run` / `fleet daemon run` are Plan 3 placeholders:

```ini
ExecStart=@BIN_DIR@/fleet-admin --config @ETC_DIR@/admin.toml serve --web-dir @WEB_DIR@
ExecStart=@BIN_DIR@/fleet-daemon --config @ETC_DIR@/daemon.toml serve
```

- Keep `--web-dir @WEB_DIR@` so the installed SPA is served.
- `Description=fleet ...` remains.
- Replace `@ADMIN_STATE_DIR@` / `@DAEMON_STATE_DIR@` defaults with
  `/var/lib/fleet/admin` and `/var/lib/fleet/daemon` in the installer template
  variables.
- Ensure `EnvironmentFile` uses `FLEET_ADMIN_TOKEN`.
- Plan 3 switches these lines to `fleet admin run` / `fleet daemon run` after
  those commands exist.

- [ ] **Step 2: Update installer binaries, users, paths, and commands**

In `scripts/install/install.sh`:

```bash
ADMIN_STATE_DIR="/var/lib/fleet/admin"
DAEMON_STATE_DIR="/var/lib/fleet/daemon"
```

Default users:

```bash
ADMIN_USER="${LEGACY_USER:-fleet-admin}"
DAEMON_USER="${LEGACY_USER:-fleet-daemon}"
```

Binaries:

```bash
for bin in fleet-admin fleet-daemon fleet; do
  src="$ROOT/target/release/$bin"
  [ -x "$src" ] || die "missing $src; run 'pnpm build:release' first"
done
```

Install:

```bash
install -m 0755 "$ROOT/target/release/fleet-admin" "$BIN_DIR/fleet-admin"
install -m 0755 "$ROOT/target/release/fleet-daemon" "$BIN_DIR/fleet-daemon"
install -m 0755 "$ROOT/target/release/fleet" "$BIN_DIR/fleet"
```

Unit installation: keep the existing `render_unit` flow. Change the source
paths to `packaging/systemd/fleet-admin.service.in` and
`packaging/systemd/fleet-daemon.service.in`, and write the rendered units to
`$SYSTEMD_DIR/fleet-admin.service` / `fleet-daemon.service`. Do not install the
`.in` templates directly.

Update systemd enable commands to `fleet-admin` / `fleet-daemon`.

Update the pairing command hint:

```bash
printf "sudo env HOME=%q FLEET_ADMIN_TOKEN='<admin-token>' %q --admin-url %q daemon setup --org org_default --server %q --daemon-config %q --name '<machine-name>'" \
  "$DAEMON_HOME" "$BIN_DIR/fleet" "http://127.0.0.1:$ADMIN_PORT" \
  "ws://127.0.0.1:$ADMIN_PORT/ws/machine" "$ETC_DIR/daemon.toml"
```

Update database names in generated configs:

```toml
database = "$ADMIN_STATE_DIR/fleet-admin.db"
database = "$DAEMON_STATE_DIR/fleet-daemon.db"
key_file = "$DAEMON_STATE_DIR/machine.key"
```

Update env file variable names to `FLEET_ADMIN_TOKEN`.

- [ ] **Step 2b: Remove legacy binaries, units, tokens, and migration paths**

Plan 1 is a clean break. Add removal/disable steps **after the existing
`DRY_RUN` early exit** and guard every destructive/systemctl action:

```bash
# Remove old binaries from the target bin directory. Safe in staged installs;
# never runs in --dry-run because the script has already exited.
# Legacy cleanup is the only place allowed to name af/agent-admin. Every such
# line carries the `fleet-legacy-cleanup` marker; verification scans filter
# lines containing that marker.
if [ "$DRY_RUN" -eq 0 ]; then
  rm -f "$BIN_DIR/af" "$BIN_DIR/agent-admin" # fleet-legacy-cleanup
fi

# Only touch host units when systemd is enabled and this is not a staged root.
if [ "$INSTALL_SYSTEMD" -eq 1 ] && [ -z "$ROOT_DIR" ]; then
  systemctl disable --now agent-admin agent-daemon 2>/dev/null || true # fleet-legacy-cleanup
  rm -f "$SYSTEMD_DIR/agent-admin.service" "$SYSTEMD_DIR/agent-daemon.service" # fleet-legacy-cleanup
  systemctl daemon-reload 2>/dev/null || true
fi
```

For `--root` staged installs, the old unit files under `$SYSTEMD_DIR` may also
be removed when `$INSTALL_SYSTEMD=1`; use `$SYSTEMD_DIR`, never a hard-coded
`/etc/systemd/system`.

Remove the legacy state placeholder completely:

- In `packaging/systemd/fleet-admin.service.in`, change
  `ReadWritePaths=@ADMIN_STATE_DIR@ -@LEGACY_STATE_DIR@` to
  `ReadWritePaths=@ADMIN_STATE_DIR@`.
- In `scripts/install/install.sh`, delete the `LEGACY_STATE_DIR` variable and
  its `render_unit` substitution (`@LEGACY_STATE_DIR@`).
- Update `scripts/test/install_check.sh` so it no longer expects
  `LEGACY_STATE_DIR` and so the `@[A-Z_]+@` placeholder assertion passes.

Change the admin token generator:

```bash
ADMIN_TOKEN_VAL="$(generate_token fla)" \
  || die "cannot generate an admin token; install openssl or write FLEET_ADMIN_TOKEN to $ETC_DIR/admin.env"
```

Remove legacy-state handling from the installer:

```bash
rg -n 'LEGACY_STATE_DIR|prepare_legacy_state|backup_existing|ACTIVE_SERVICES|\.agent-fleet' scripts/install/install.sh
```

For each hit:

- delete the legacy state directory variable and any migration/copy logic;
- update `daemon_key_file`'s fallback from `$DAEMON_HOME/.agent-fleet/machine.key`
  to `$DAEMON_STATE_DIR/machine.key` (or keep the explicit `key_file` from
  `daemon.toml`); do not append `.fleet/daemon` to `$DAEMON_HOME`;
- rename `ACTIVE_SERVICES` entries and the wait/health-check loop from
  `agent-admin`/`agent-daemon` to `fleet-admin`/`fleet-daemon`;
- keep a `backup_existing` helper only if it backs up the new config/state
  files; delete it if it only copies pre-split legacy state;
- update comments that describe upgrading pre-split installs; the new installer
  is fresh-install only.

Update the automatic pairing invocation:

```bash
HOME="$DAEMON_HOME" FLEET_ADMIN_TOKEN="$ADMIN_TOKEN_VAL" \
  "$BIN_DIR/fleet" --admin-url "http://127.0.0.1:$ADMIN_PORT" \
  daemon setup --org org_default \
  --server "ws://127.0.0.1:$ADMIN_PORT/ws/machine" \
  --daemon-config "$ETC_DIR/daemon.toml"
```

- [ ] **Step 3: Update `package.json` scripts**

```json
"dev": "VITE_DEFAULT_API_TOKEN=${FLEET_ADMIN_TOKEN:-dev-token} pnpm run build:web && pnpm run dev:admin",
"dev:admin": "FLEET_ADMIN_TOKEN=${FLEET_ADMIN_TOKEN:-dev-token} ${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo} run -p fleet-admin --bin fleet-admin -- serve",
"dev:daemon": "${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo} run -p fleet-daemon -- serve",
"dev:web": "VITE_DEFAULT_API_TOKEN=${FLEET_ADMIN_TOKEN:-dev-token} pnpm --filter @agent-fleet/web dev",
"fleet": "${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo} run -q -p fleet-cli --bin fleet",
"install:local": "./scripts/install/install.sh"
```

Remove `AGENT_FLEET_*` from all scripts, including `dev` and `dev:web`.

- [ ] **Step 4: Verify shell and package files**

Run:
```bash
bash -n scripts/install/install.sh
node -e "JSON.parse(require('fs').readFileSync('package.json'))"
rg -n --hidden 'AGENT_FLEET_|agent-admin|\baf\b|\.agent-fleet|af_admin_' \
  scripts/install/install.sh packaging/systemd package.json \
  | grep -v 'fleet-legacy-cleanup'
```
Expected: both commands exit 0; the filtered scan has no output. Only the
marked legacy-cleanup lines may name `af`/`agent-admin`.

- [ ] **Step 5: Commit**

```bash
git add package.json scripts/install/install.sh packaging/systemd
git commit -m "refactor(packaging): install fleet, fleet-admin, and fleet-daemon" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 11: Update E2E scripts and shell helpers

**Files:**
- Modify: `scripts/test/install_check.sh`
- Modify: `scripts/test/docker_verify.sh`
- Modify: `scripts/test/git_write_e2e.sh`
- Modify: `scripts/test/multi_machine_stress.sh`
- Modify: `scripts/test/ui_e2e.sh`
- Modify: `scripts/test/e2e_smoke.sh`

- [ ] **Step 1: Scan for old names**

```bash
rg -n --hidden 'AF_BIN|target/debug/af|\baf\b|AGENT_FLEET_|agent-admin|~/.agent-fleet|\.agent-fleet|--url|--token|X-Agent-Fleet|agent-admin\.db|agent\.db' scripts/test
```

- [ ] **Step 2: Apply the command and flag mapping**

Replace:

- `AF_BIN` → `FLEET_BIN`
- `target/debug/af` → `target/debug/fleet`
- `build -p fleet-cli --bin af` → `build -p fleet-cli --bin fleet`
- `"$AF_BIN" ... setup` → `"$FLEET_BIN" ... daemon setup`
- `"$AF_BIN" ... machines ...` → `"$FLEET_BIN" ... admin machines ...`
- `"$AF_BIN" ... config ...` → `"$FLEET_BIN" ... config ...`
- `--url` → `--admin-url`
- `--token` → `--admin-token` (for `fleet` commands; do not change HTTP
  `Authorization: Bearer` headers)
- `target/debug/agent-admin` → `target/debug/fleet-admin`
- `AGENT_FLEET_E2E_TOKEN` → `FLEET_E2E_TOKEN`
- `AGENT_FLEET_E2E_WRITE_TOKEN` → `FLEET_E2E_WRITE_TOKEN`
- `AGENT_FLEET_ADMIN_TOKEN` → `FLEET_ADMIN_TOKEN`
- `AGENT_FLEET_CONFIG` → `FLEET_CONFIG`
- `AGENT_FLEET_GIT_WRITE_TOKEN` → `FLEET_GIT_WRITE_TOKEN`
- `X-Agent-Fleet-Git-Token` → `X-Fleet-Git-Token`
- `~/.agent-fleet` and `.agent-fleet` → `~/.fleet` / `.fleet`
- temp DBs: `agent-admin.db` → `fleet-admin.db`,
  `agent.db` → `fleet-daemon.db`
- log filenames: `agent-admin.log` → `fleet-admin.log`,
  `agent-daemon.log` → `fleet-daemon.log`

Pay special attention to:
`git_write_e2e.sh`, `ui_e2e.sh`, `multi_machine_stress.sh`, and the smoke
script. The multi-machine test must use `fleet daemon setup` with a pairing
code, not a shared machine token.

- [ ] **Step 2b: Update `install_check.sh` assertions**

`scripts/test/install_check.sh` hard-codes the old install shape. Update:

- binaries: `fleet`, `fleet-admin`, `fleet-daemon`;
- units: `fleet-admin.service`, `fleet-daemon.service`;
- `ExecStart=` lines to the Plan 1 direct-binary form;
- service users `fleet-admin` / `fleet-daemon`;
- state paths `/var/lib/fleet/admin`, `/var/lib/fleet/daemon`;
- env keys `FLEET_ADMIN_TOKEN`;
- database names `fleet-admin.db`, `fleet-daemon.db`;
- no `af` or `agent-admin` assertions.

Run:
```bash
rg -n 'agent-admin|agent-daemon|\baf\b|AGENT_FLEET_|/var/lib/fleet-admin|/var/lib/fleet-daemon' scripts/test/install_check.sh
```
Expected: no output.

- [ ] **Step 3: Verify**

Run:
```bash
rg -n --hidden 'AF_BIN|target/debug/af|AGENT_FLEET_|agent-admin|~/.agent-fleet|\.agent-fleet|X-Agent-Fleet|agent-admin\.db|agent\.db|--url|--token' scripts/test
bash -n scripts/test/*.sh
```
Expected: no old-name/flag hits; all scripts parse.

- [ ] **Step 4: Commit**

```bash
git add scripts/test
git commit -m "test(e2e): run fleet admin/daemon through the new CLI" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 12: Update user documentation and examples

**Files:**
- Modify: `README.md`
- Modify: `docs/development.md`
- Modify: `docs/connectivity.md`
- Modify: `docs/security.md`
- Modify: `docs/crate-interfaces.md`
- Modify: `docs/mvp-status.md`
- Modify: `docs/architecture.md`
- Modify: `docs/implementation.md`
- Modify: `docs/protocols.md`
- Modify: `apps/docs/src/content/docs/**`
- Modify: `crates/core-config/examples/*.toml`
- Modify: `.env.example`

- [ ] **Step 1: Scan for old names**

```bash
rg -n --hidden '\baf\b|agent-admin|AGENT_FLEET_|~/.agent-fleet|\.agent-fleet|af_admin_|afm_|afu_|agent-admin\.db|agent\.db|X-Agent-Fleet|--url|--token' \
  README.md docs apps/docs crates/core-config/examples .env.example \
  --glob '!docs/superpowers/**'
```

- [ ] **Step 2: Apply the mapping**

Use the command mapping from the spec:

| Old | New |
|---|---|
| `af setup` | `fleet daemon setup` |
| `af machines ...` | `fleet admin machines ...` |
| `af workspaces ...` | `fleet admin workspaces ...` |
| `af sessions ...` | `fleet admin sessions ...` |
| `af users ...` | `fleet admin users ...` |
| `af orgs ...` | `fleet admin orgs ...` |
| `af audit` | `fleet admin audit` |
| `af permissions ...` | `fleet admin permissions ...` |
| `af health` | `fleet admin health` |
| `af config ...` | `fleet config ...` |
| `af network ...` | `fleet network ...` |
| `agent-admin` | `fleet-admin` |
| `AGENT_FLEET_*` | `FLEET_*` |
| `~/.agent-fleet` | `~/.fleet` |
| `agent-admin.db` | `fleet-admin.db` |
| `agent.db` | `fleet-daemon.db` |
| `af_admin_` | `fla_` |
| `afm_` | `flm_` |
| `afu_` | `flu_` |

Do not put `fleet admin setup`, `fleet admin start`, or `fleet daemon start`
in the quick start as working commands yet. Plan 1 only lands the command
tree; those commands return "implemented in a later plan".

Update the README quick start to the commands that work after Plan 1:

```bash
# Start the admin directly until Plan 3 adds `fleet admin start`.
FLEET_ADMIN_TOKEN=<admin-token> \
  fleet-admin --config ~/.fleet/admin/config.toml serve --web-dir apps/web/dist

# Store the admin URL/token for the CLI and create the daemon config.
fleet config init --admin-url http://127.0.0.1:8787 \
  --admin-token <admin-token>

# Plan 1 daemon setup uses an admin token to create the pairing code.
fleet daemon setup --admin-url http://127.0.0.1:8787 \
  --server ws://127.0.0.1:8787/ws/machine \
  --org org_default --name dev-machine --store-token

# Split flow, redeeming a code created by `fleet admin machines pair-code`:
fleet-daemon pair --server ws://127.0.0.1:8787/ws/machine --code <code>

# Admin REST commands now use ~/.fleet/cli.toml.
fleet admin machines list
fleet config show
fleet network detect
```

Add a short "Coming next" note that `fleet admin setup`, `fleet admin start`,
`fleet daemon start`, `fleet status`, and `fleet doctor` arrive in follow-up
plans.

Remove migration wording that contradicts the clean break. In particular:

- `docs/development.md`: remove "Existing deployments can rename the file or
  pass `--db ...`" and similar pre-split migration notes.
- `README.md`: remove statements that pre-split configs keep working or that
  old state is migrated.
- Replace them with: "This iteration has no backward compatibility; start from
  `~/.fleet`."

- [ ] **Step 3: Verify**

```bash
rg -n --hidden '\baf\b|agent-admin|AGENT_FLEET_|~/.agent-fleet|\.agent-fleet|af_admin_|afm_|afu_|agent-admin\.db|agent\.db|X-Agent-Fleet|--url|--token' \
  README.md docs apps/docs crates/core-config/examples .env.example \
  --glob '!docs/superpowers/**'
```
Expected: no operational hits. Product-prose "agent-fleet"/"Agent Fleet" and
npm package names are allowed by the rebrand boundary. `--url`/`--token` may
appear in generic HTTP prose; rewrite CLI examples to `--admin-url` /
`--admin-token`.

- [ ] **Step 4: Commit**

```bash
git add README.md docs apps/docs crates/core-config/examples
git commit -m "docs: update commands, paths, tokens, and environment names" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 13: Full verification and cleanup

**Files:**
- All touched files

- [ ] **Step 1: Format**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo fmt --all
```

- [ ] **Step 2: Run the workspace tests**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo test --workspace
```
Expected: PASS. If a test still uses old names, update the test and rerun.

- [ ] **Step 3: Run clippy**

```bash
CARGO_BUILD_JOBS=4 /home/luozhouyang/Applications/pcargo/cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no warnings.

- [ ] **Step 4: Run the web checks**

```bash
pnpm run test:web
```
Expected: PASS. The web app does not use the CLI names, but the docs workspace
may be affected by docs edits.

- [ ] **Step 4b: Run the installer check**

```bash
./scripts/test/install_check.sh
```
Expected: PASS. This catches unrendered `@...@` placeholders, dry-run deletion
bugs, old unit names, and wrong `ExecStart` lines.

- [ ] **Step 5: Final legacy-name scan**

```bash
rg -n --hidden '\baf\b|agent-admin|agent-daemon|AGENT_FLEET_|~/.agent-fleet|\.agent-fleet|af_admin_|afm_|afu_|agent-admin\.db|agent\.db|X-Agent-Fleet|--url|--token' \
  apps crates packages scripts packaging docs README.md package.json .env.example \
  --glob '!docs/superpowers/**' \
  --glob '!target/**' \
  | grep -v 'fleet-legacy-cleanup'
```
Expected: no operational hits outside the marked legacy-cleanup lines.
Product-prose "agent-fleet"/"Agent Fleet" and npm package names are allowed by
the rebrand boundary. Rewrite CLI examples that still use `--url`/`--token`.

- [ ] **Step 5b: Fix any scan hits before continuing**

For every hit:

- operational path/env/token/binary/header → rename;
- product prose or package name → leave it, and confirm it is covered by the
  rebrand boundary in this plan.

Re-run Step 5 until it is clean.

- [ ] **Step 6: Smoke test the new command tree**

```bash
./target/debug/fleet --help
./target/debug/fleet admin --help
./target/debug/fleet daemon --help
./target/debug/fleet config path
./target/debug/fleet network detect
```
Expected:
- `fleet admin` lists machines/workspaces/sessions/users/orgs/audit/permissions
  plus placeholder process commands;
- `fleet daemon` lists setup/pair/config/process placeholders;
- `fleet config path` prints `~/.fleet/cli.toml`;
- `network detect` still works (it may report missing tooling).

- [ ] **Step 7: Commit**

Review the diff first:

```bash
git status --short
git diff --stat
```

Add the touched paths explicitly (adjust the list to the actual diff):

```bash
git add apps crates packages scripts packaging docs README.md package.json .env.example
git commit -m "chore: verify fleet rebrand and command tree" \
  -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Plan 1 acceptance checklist

- [ ] `fleet`, `fleet-admin`, and `fleet-daemon` build.
- [ ] `fleet-admin` and `fleet-daemon` are thin binaries over library crates.
- [ ] Token generation uses `fla_`, `flm_`, and `flu_`.
- [ ] All environment variables use `FLEET_*`.
- [ ] Default paths use `~/.fleet/...` and databases are
      `fleet-admin.db` / `fleet-daemon.db`.
- [ ] `fleet admin machines ...`, `fleet daemon setup ...`,
      `fleet config ...`, and `fleet network ...` work.
- [ ] New process/setup/status/doctor commands parse and return clear
      "implemented in a later plan" errors.
- [ ] `cargo test --workspace`, clippy, and `pnpm run test:web` pass.
- [ ] No non-historical `af` / `agent-admin` / `AGENT_FLEET_*` /
      `af_admin_` / `afm_` / `afu_` / `~/.agent-fleet` references remain,
      except the installer lines marked `# fleet-legacy-cleanup`.

## Execution handoff

Plan complete and saved to
`docs/superpowers/plans/2026-11-16-fleet-rebrand.md`.

Two execution options:

1. **Subagent-Driven (recommended):** use
   `superpowers:subagent-driven-development` with a fresh subagent per task and
   review between tasks.
2. **Inline Execution:** use `superpowers:executing-plans` in this session with
   checkpoints.

Which approach?

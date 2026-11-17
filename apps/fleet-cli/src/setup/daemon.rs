//! `fleet daemon setup` — configure the daemon and pair this machine.
//!
//! Planning is side-effect free. Setup loads the existing daemon configuration
//! without creating directories, resolves the database/key paths with the same
//! precedence as the admin setup, rejects a database shared with the admin and
//! validates the WebSocket URL. Only [`run`] writes files, pairs, registers
//! workspaces or installs the unit.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;
use fleet_core_auth::{
    load_or_create_machine_id, machine_token_path, normalize_pair_code, MachineIdentity,
};
use fleet_core_config::{DaemonConfig, ENV_DATA_DIR, ENV_DB};
use fleet_core_protocol::pairing::{CreatePairingCodeRequest, PairingCodeResponse};
use serde_json::Value;

use crate::client::AdminClient;
use crate::config::{
    create_private_dir, default_admin_config_path, edit_toml_with_removals, expand_tilde,
    CliConfig, TomlEdit,
};
use crate::config_paths::{ensure_distinct_databases, RolePaths};
use crate::pairing::{derive_ws_url, validate_server_url};
use crate::process::service;
use crate::prompt::{self, PromptOptions};

/// Flags for `fleet daemon setup`.
#[derive(Debug, Clone, Default, Args)]
pub struct DaemonSetupArgs {
    /// Organization id (default: the admin's default organization).
    #[arg(long)]
    pub org: Option<String>,
    /// Admin WebSocket URL (default: derived from the admin URL).
    #[arg(long)]
    pub server: Option<String>,
    /// Short-lived pairing code to redeem.
    #[arg(long)]
    pub code: Option<String>,
    /// Machine display name.
    #[arg(long)]
    pub name: Option<String>,
    /// Ed25519 machine key file (default: <state dir>/machine.key).
    #[arg(long)]
    pub key_file: Option<PathBuf>,
    /// SQLite database path (default: <state dir>/fleet-daemon.db).
    #[arg(long)]
    pub database: Option<PathBuf>,
    /// Local workspace root to register after setup (repeatable).
    #[arg(long = "workspace", value_name = "PATH")]
    pub workspace: Vec<PathBuf>,
    /// Save the admin token and daemon config path to the CLI configuration.
    #[arg(long)]
    pub store_token: bool,
    /// Only write configuration; do not redeem a code or pair.
    #[arg(long)]
    pub configure_only: bool,
    /// Allow plaintext ws:// on a non-loopback host (trusted LAN only).
    #[arg(long)]
    pub allow_insecure: bool,
    /// Install the platform service.
    #[arg(long, conflicts_with = "no_install_service")]
    pub install_service: bool,
    /// Do not install the platform service.
    #[arg(long)]
    pub no_install_service: bool,
    /// Start the role after setup (native service when installed, otherwise the supervisor).
    #[arg(long)]
    pub start: bool,
    /// Accept the defaults without prompting.
    #[arg(long)]
    pub yes: bool,
    /// Print the plan without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Inputs for [`run`]. Paths are resolved by [`crate::cli::RoleArgs::setup_paths`].
pub struct DaemonSetupOptions {
    pub paths: RolePaths,
    pub args: DaemonSetupArgs,
    pub admin_url: String,
    pub admin_token: Option<String>,
    pub prompt: PromptOptions,
    /// CLI configuration written by `--store-token` (the active `--config`).
    pub cli_config_path: PathBuf,
}

/// How this run obtains the per-machine token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Redeem a short-lived pairing code.
    Code(String),
    /// Ask the admin for a pairing code using an admin token.
    AdminToken(String),
    /// Write configuration only.
    ConfigureOnly,
}

/// Resolve and validate the pairing mode before any identity file is created.
pub fn choose_mode(
    args: &DaemonSetupArgs,
    admin_token: Option<String>,
    prompt: PromptOptions,
) -> Result<Mode> {
    if args.configure_only {
        return Ok(Mode::ConfigureOnly);
    }
    if let Some(code) = &args.code {
        return Ok(Mode::Code(normalize_code(code)?));
    }
    if prompt.interactive() {
        let choice = prompt::select(
            prompt,
            "Pair how?",
            &[
                "Redeem a pairing code (recommended for worker machines)",
                "Use an admin token (same host / trusted LAN)",
                "Configure only (already paired)",
            ],
            0,
        )?;
        return match choice {
            0 => Ok(Mode::Code(normalize_code(&prompt::secret(
                "Pairing code",
            )?)?)),
            1 => {
                let token = match admin_token {
                    Some(token) if !token.trim().is_empty() => token,
                    _ => prompt::secret("Admin token")?,
                };
                require_admin_token(token)
            }
            _ => Ok(Mode::ConfigureOnly),
        };
    }
    if let Some(token) = admin_token.filter(|token| !token.trim().is_empty()) {
        return Ok(Mode::AdminToken(token));
    }
    bail!(
        "choose a pairing mode: pass --code, --configure-only, or supply an admin token with --admin-token or FLEET_ADMIN_TOKEN"
    )
}

fn require_admin_token(token: String) -> Result<Mode> {
    if token.trim().is_empty() {
        bail!("admin token must not be empty");
    }
    Ok(Mode::AdminToken(token))
}

fn normalize_code(code: &str) -> Result<String> {
    normalize_pair_code(code).ok_or_else(|| {
        anyhow::anyhow!("invalid pairing code; expected 12 Crockford Base32 symbols")
    })
}

/// Configure the daemon, pair this machine and optionally install the unit.
pub async fn run(options: DaemonSetupOptions) -> Result<()> {
    let paths = &options.paths;
    let args = &options.args;
    let config_existed = paths.config.exists();

    // Load without `resolve_paths`: resolving expands `~` and creates
    // directories, and setup must keep `--dry-run` side-effect free.
    let loaded = DaemonConfig::load(Some(&paths.config))
        .with_context(|| format!("failed to load {}", paths.config.display()))?;

    let env_database = std::env::var_os(ENV_DB).is_some();
    let env_data_dir = std::env::var_os(ENV_DATA_DIR).is_some();

    // `--database` → the loaded value when the file exists or an env override
    // is active → the role's resolved default. This keeps a brand-new
    // system-scope config out of `~/.fleet` while honoring explicit overrides.
    let database = match &args.database {
        Some(path) => path.clone(),
        None if config_existed || env_database || env_data_dir => loaded.storage.database.clone(),
        None => paths.database.clone(),
    };
    let database = expand_tilde(database);

    // `--key-file` → the loaded value when the file exists or `FLEET_DATA_DIR`
    // is active → `<state dir>/machine.key`. `FLEET_DB` does not affect keys.
    let key_file = match &args.key_file {
        Some(path) => path.clone(),
        None if config_existed || env_data_dir => loaded.machine.key_file.clone(),
        None => paths.state_dir.join("machine.key"),
    };
    let key_file = expand_tilde(key_file);

    // Reject a database shared with the admin. The daemon config path comes
    // from `cli.toml`, or the documented default. Loading never writes.
    let cli_config = CliConfig::load(&options.cli_config_path)?;
    let admin_config = cli_config
        .admin_config
        .clone()
        .unwrap_or_else(default_admin_config_path);
    let admin = fleet_core_config::AdminConfig::load(Some(&admin_config))
        .with_context(|| format!("failed to load {}", admin_config.display()))?;
    ensure_distinct_databases(&expand_tilde(admin.storage.database), &database)?;

    let server_url = args
        .server
        .clone()
        .unwrap_or(derive_ws_url(&options.admin_url)?);
    validate_server_url(&server_url, args.allow_insecure)?;

    let display_name = args
        .name
        .clone()
        .or_else(|| loaded.machine.name.clone())
        .unwrap_or_else(fleet_core_connection::default_machine_name);

    if options.prompt.dry_run {
        print_dry_run(&options, &server_url, &database, &key_file, &display_name);
        return Ok(());
    }

    // Mode validation happens before identity files: `--yes` with no code,
    // token or `--configure-only` must not leave a key behind.
    let mode = choose_mode(args, options.admin_token.clone(), options.prompt)?;

    for path in [&key_file, &database] {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                create_private_dir(parent)?;
            }
        }
    }

    let identity = MachineIdentity::load_or_create(&key_file)?;
    let machine_id = match loaded.machine.id.clone() {
        Some(id) => id,
        None => load_or_create_machine_id(&key_file, &identity)?,
    };
    let machine_name = args
        .name
        .clone()
        .or_else(|| loaded.machine.name.clone())
        .unwrap_or_else(fleet_core_connection::default_machine_name);

    match &mode {
        Mode::Code(code) => {
            let paired = crate::pairing::pair_with_code(
                &server_url,
                code,
                &machine_id,
                &machine_name,
                &key_file,
            )
            .await?;
            println!("paired machine {paired}");
        }
        Mode::AdminToken(token) => {
            let client = AdminClient::new(&options.admin_url, Some(token.clone()))?;
            client
                .health()
                .await
                .context("fleet admin is not reachable")?;
            let value = client
                .post(
                    "/api/v1/pairing/codes",
                    serde_json::to_value(CreatePairingCodeRequest {
                        org_id: args.org.clone(),
                        machine_name: Some(machine_name.clone()),
                    })?,
                )
                .await?;
            let response: PairingCodeResponse = serde_json::from_value(value)?;
            let paired = crate::pairing::pair_with_code(
                &server_url,
                &response.code,
                &machine_id,
                &machine_name,
                &key_file,
            )
            .await?;
            println!("paired machine {paired}");
        }
        Mode::ConfigureOnly => {}
    }

    write_daemon_config(
        &paths.config,
        &server_url,
        &database,
        &key_file,
        &machine_id,
        Some(machine_name.as_str()),
    )?;

    if args.store_token {
        store_cli_token(&options, &mode)?;
    }

    for root in &args.workspace {
        let root = expand_tilde(root.clone());
        let value = run_daemon_json(
            paths,
            &["register-workspace".to_string(), root.display().to_string()],
        )
        .await?;
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| root.to_str().unwrap_or("workspace"));
        println!("registered workspace {name}");
    }

    let token_present = machine_token_path(&key_file).exists();
    let install_service = match (args.install_service, args.no_install_service) {
        (true, _) => true,
        (false, true) => false,
        (false, false) if options.prompt.interactive() => prompt::confirm(
            options.prompt,
            &format!(
                "Install and start the {} service?",
                service::prompt_label_for(paths)
            ),
            service::available_for(paths).is_some(),
        )?,
        (false, false) => service::available_for(paths).is_some(),
    };

    let mut skipped = false;
    if install_service {
        // The daemon must be paired before the service starts it. Configure-only
        // with no token therefore installs the unit stopped.
        let outcome = service::install(paths, None, token_present, false, options.prompt.yes)?;
        skipped = outcome.skipped;
        if !skipped {
            println!("installed {}", outcome.unit_path.display());
        }
    }

    let installation_skipped = !install_service || skipped;
    if !token_present {
        println!(
            "next: pair this machine with `fleet daemon pair --server {server_url} --code <code>`"
        );
    }
    if installation_skipped && args.start {
        println!("starting the daemon with the fallback supervisor");
        crate::process::start_role(paths, crate::cli::ProcessMode::Supervisor, 15).await?;
    } else if installation_skipped && token_present {
        println!("next: start the daemon with `fleet daemon start`");
    } else if token_present && !installation_skipped {
        println!("daemon service installed and started");
    }

    Ok(())
}

fn print_dry_run(
    options: &DaemonSetupOptions,
    server_url: &str,
    database: &Path,
    key_file: &Path,
    name: &str,
) {
    let args = &options.args;
    println!("dry run; nothing will be written");
    println!("daemon config  {}", options.paths.config.display());
    println!("server         {server_url}");
    println!("database       {}", database.display());
    println!("key file       {}", key_file.display());
    println!("machine name   {name}");
    println!(
        "mode           {}",
        if args.configure_only {
            "configure only"
        } else if args.code.is_some() {
            "pairing code"
        } else if options.admin_token.is_some() {
            "admin token"
        } else {
            "prompt"
        }
    );
    println!(
        "store token    {}",
        if args.store_token { "yes" } else { "no" }
    );
    println!(
        "service        {}",
        if args.install_service {
            "install"
        } else if args.no_install_service {
            "skip"
        } else {
            "install when a native service manager is available"
        }
    );
}

fn write_daemon_config(
    path: &Path,
    server_url: &str,
    database: &Path,
    key_file: &Path,
    machine_id: &str,
    name: Option<&str>,
) -> Result<()> {
    let mut edits = vec![
        TomlEdit::string("connection", "server_url", server_url),
        TomlEdit::string("machine", "id", machine_id),
        TomlEdit::string("machine", "key_file", key_file.display().to_string()),
        TomlEdit::string("storage", "database", database.display().to_string()),
    ];
    if let Some(name) = name {
        edits.push(TomlEdit::string("machine", "name", name));
    }
    let backup = edit_toml_with_removals(
        path,
        &edits,
        &[("connection", "token"), ("connection", "pairing_code")],
    )?;

    // Fail loudly if a future edit would produce a config the daemon rejects.
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    DaemonConfig::from_str(&raw)
        .with_context(|| format!("invalid daemon config {}", path.display()))?;

    println!(
        "wrote {}{}",
        path.display(),
        backup
            .map(|backup| format!(" (backup: {})", backup.display()))
            .unwrap_or_default()
    );
    Ok(())
}

fn store_cli_token(options: &DaemonSetupOptions, mode: &Mode) -> Result<()> {
    let token = match mode {
        Mode::AdminToken(token) => Some(token.clone()),
        _ => options.admin_token.clone(),
    }
    .filter(|token| !token.trim().is_empty());
    let Some(token) = token else {
        // Pairing-code and configure-only runs have no admin token to store.
        return Ok(());
    };

    let mut cli = CliConfig::load(&options.cli_config_path)?;
    cli.token = Some(token);
    cli.admin_url = Some(options.admin_url.clone());
    cli.daemon_config = Some(options.paths.config.clone());
    cli.save(&options.cli_config_path)?;
    println!("wrote {}", options.cli_config_path.display());
    Ok(())
}

/// Run a `fleet-daemon` subcommand that prints JSON and parse the result.
///
/// The daemon owns local workspace registration, so setup invokes the resolved
/// server binary instead of duplicating storage logic.
async fn run_daemon_json(paths: &RolePaths, args: &[String]) -> Result<Value> {
    let binary = paths.require_binary()?;
    let output = tokio::process::Command::new(binary)
        .arg("--config")
        .arg(&paths.config)
        .args(args)
        .output()
        .await
        .with_context(|| format!("failed to run {}", binary.display()))?;
    if !output.status.success() {
        bail!(
            "{} exited with {}: {}",
            binary.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{} returned invalid JSON", binary.display()))
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)] // tests serialize process-wide env mutations
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::config_paths::Role;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_VARS: &[&str] = &[
        "FLEET_DB",
        "FLEET_DATA_DIR",
        "FLEET_MACHINE_NAME",
        "FLEET_SERVER_URL",
        "FLEET_PAIR_CODE",
        "FLEET_BIND",
        "FLEET_GIT_WRITES",
        "FLEET_ADMIN_TOKEN",
        "FLEET_CONFIG",
        "FLEET_CLI_CONFIG",
    ];

    /// Serialize env-mutating tests and hide the developer's environment.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        for key in ENV_VARS {
            std::env::remove_var(key);
        }
        guard
    }

    fn test_paths(dir: &Path) -> RolePaths {
        RolePaths {
            role: Role::Daemon,
            config: dir.join("daemon/config.toml"),
            state_dir: dir.join("daemon/state"),
            database: dir.join("daemon/state/fleet-daemon.db"),
            binary: None,
            fleet_binary: PathBuf::from("/usr/local/bin/fleet"),
            service_user: "tester".to_string(),
            service_group: "tester".to_string(),
            web_dir: None,
        }
    }

    fn configure_only_args() -> DaemonSetupArgs {
        DaemonSetupArgs {
            configure_only: true,
            no_install_service: true,
            yes: true,
            ..DaemonSetupArgs::default()
        }
    }

    fn options(dir: &Path, paths: RolePaths, args: DaemonSetupArgs) -> DaemonSetupOptions {
        DaemonSetupOptions {
            paths,
            args,
            admin_url: "http://127.0.0.1:9".to_string(),
            admin_token: None,
            prompt: PromptOptions {
                yes: true,
                dry_run: false,
            },
            cli_config_path: dir.join("cli.toml"),
        }
    }

    #[test]
    fn mode_selection_prefers_configure_only_then_code() {
        let args = DaemonSetupArgs {
            configure_only: true,
            ..DaemonSetupArgs::default()
        };
        assert_eq!(
            choose_mode(
                &args,
                None,
                PromptOptions {
                    yes: true,
                    dry_run: false
                }
            )
            .unwrap(),
            Mode::ConfigureOnly
        );

        let args = DaemonSetupArgs {
            code: Some("abcd-efgh-jkmn".into()),
            ..DaemonSetupArgs::default()
        };
        assert_eq!(
            choose_mode(
                &args,
                None,
                PromptOptions {
                    yes: true,
                    dry_run: false
                }
            )
            .unwrap(),
            Mode::Code(normalize_pair_code("abcd-efgh-jkmn").unwrap())
        );
    }

    #[test]
    fn mode_selection_uses_supplied_admin_token_without_prompting() {
        let args = DaemonSetupArgs::default();
        let mode = choose_mode(
            &args,
            Some("fla_test".into()),
            PromptOptions {
                yes: true,
                dry_run: false,
            },
        )
        .unwrap();
        assert_eq!(mode, Mode::AdminToken("fla_test".into()));
    }

    #[test]
    fn mode_selection_errors_for_non_interactive_runs_without_a_mode() {
        let args = DaemonSetupArgs {
            yes: true,
            ..DaemonSetupArgs::default()
        };
        let error = choose_mode(
            &args,
            None,
            PromptOptions {
                yes: true,
                dry_run: false,
            },
        )
        .expect_err("no pairing mode");
        assert!(error.to_string().contains("choose a pairing mode"));
    }

    #[test]
    fn mode_selection_rejects_invalid_codes() {
        let args = DaemonSetupArgs {
            code: Some("short".into()),
            ..DaemonSetupArgs::default()
        };
        assert!(choose_mode(
            &args,
            None,
            PromptOptions {
                yes: true,
                dry_run: false
            }
        )
        .is_err());
    }

    #[tokio::test]
    async fn configure_only_writes_config_and_removes_legacy_fields() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let paths = test_paths(dir.path());
        fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        let database = dir.path().join("custom/daemon.db");
        let key_file = dir.path().join("custom/machine.key");
        fs::write(
            &paths.config,
            format!(
                "[connection]\nserver_url = \"ws://old\"\ntoken = \"legacy\"\npairing_code = \"LEGACY\"\n\n[storage]\ndatabase = \"{}\"\n\n[machine]\nkey_file = \"{}\"\n",
                database.display(),
                key_file.display()
            ),
        )
        .unwrap();

        run(options(dir.path(), paths.clone(), configure_only_args()))
            .await
            .unwrap();

        let raw = fs::read_to_string(&paths.config).unwrap();
        assert!(!raw.contains("token = \"legacy\""), "{raw}");
        assert!(!raw.contains("pairing_code"), "{raw}");
        let config = DaemonConfig::from_str(&raw).unwrap();
        assert_eq!(config.storage.database, database);
        assert_eq!(config.machine.key_file, key_file);
        assert_eq!(
            config.connection.server_url.as_deref(),
            Some("ws://127.0.0.1:9/ws/machine")
        );
        assert!(!machine_token_path(&key_file).exists());
    }

    #[tokio::test]
    async fn dry_run_writes_nothing_before_mode_validation() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let paths = test_paths(dir.path());
        // No code, token or configure-only: without `--dry-run` this errors.
        // Dry-run must print the plan and return before mode validation.
        let args = DaemonSetupArgs {
            yes: true,
            dry_run: true,
            ..DaemonSetupArgs::default()
        };
        let mut options = options(dir.path(), paths.clone(), args);
        options.prompt.dry_run = true;
        run(options).await.unwrap();

        assert!(!paths.config.exists());
        assert!(!paths.state_dir.exists());
        assert!(!dir.path().join("cli.toml").exists());
    }

    #[tokio::test]
    async fn rejects_database_collision_with_admin_config() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.db");
        let admin_config = dir.path().join("admin.toml");
        fs::write(
            &admin_config,
            format!("[storage]\ndatabase = \"{}\"\n", shared.display()),
        )
        .unwrap();
        let cli_path = dir.path().join("cli.toml");
        fs::write(
            &cli_path,
            format!("admin_config = \"{}\"\n", admin_config.display()),
        )
        .unwrap();

        let paths = test_paths(dir.path());
        let mut args = configure_only_args();
        args.database = Some(shared);
        let mut options = options(dir.path(), paths.clone(), args);
        options.cli_config_path = cli_path.clone();
        let before = fs::read_to_string(&cli_path).unwrap();

        let error = run(options).await.expect_err("colliding databases");
        assert!(error.to_string().contains("must be different"), "{error}");
        assert!(!paths.config.exists());
        assert_eq!(fs::read_to_string(&cli_path).unwrap(), before);
    }

    #[tokio::test]
    async fn new_config_uses_role_paths_not_home_defaults() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let mut paths = test_paths(dir.path());
        paths.config = dir.path().join("custom/daemon.toml");
        paths.state_dir = dir.path().join("custom/state");
        paths.database = paths.state_dir.join("fleet-daemon.db");

        run(options(dir.path(), paths.clone(), configure_only_args()))
            .await
            .unwrap();

        let raw = fs::read_to_string(&paths.config).unwrap();
        let config = DaemonConfig::from_str(&raw).unwrap();
        assert_eq!(config.storage.database, paths.database);
        assert_eq!(config.machine.key_file, paths.state_dir.join("machine.key"));
        assert!(paths.state_dir.join("machine.key").exists());
        assert!(!raw.contains(".fleet/daemon"), "{raw}");
    }
}

//! `fleet admin setup` — configure the control plane and optionally install
//! its systemd unit.
//!
//! Planning is side-effect free: it loads the existing role and CLI
//! configuration, resolves the database/token/web directory, and rejects a
//! database shared with the daemon. Only [`run`] writes files or installs the
//! service.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use fleet_core_auth::generate_admin_token;
use fleet_core_config::{AdminConfig, DaemonConfig};

use crate::client::AdminClient;
use crate::config::{
    backup_file, create_private_dir, default_daemon_config_path, edit_toml_with_removals,
    expand_tilde, CliConfig, TomlEdit,
};
use crate::config_paths::{ensure_distinct_databases, RolePaths};
use crate::process::systemd::{self, systemd_available};
use crate::prompt::{self, PromptOptions};

/// Inputs for [`run`]. Paths are resolved by [`crate::cli::RoleArgs::setup_paths`]
/// before this module is called.
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

/// Resolved setup plan. Deliberately not `Debug`: it carries the admin token.
struct AdminSetupPlan {
    host: String,
    port: u16,
    database: PathBuf,
    web_dir: Option<PathBuf>,
    token: String,
    token_generated: bool,
    write_cli_config: bool,
    install_service: bool,
    daemon_config: PathBuf,
    admin_url: String,
}

/// Resolve every value the setup would write without touching the filesystem.
fn build_plan(options: &AdminSetupOptions) -> Result<AdminSetupPlan> {
    let paths = &options.paths;
    let config_existed = paths.config.exists();
    let loaded = AdminConfig::load(Some(&paths.config))
        .with_context(|| format!("failed to load {}", paths.config.display()))?;

    // `AdminConfig::load` applies `FLEET_DB` / `FLEET_DATA_DIR`; only prefer the
    // loaded value when the file exists or an env override is active. A brand
    // new system-scope config must not inherit `~/.fleet` defaults.
    let env_database = std::env::var_os(fleet_core_config::ENV_DB).is_some();
    let env_data_dir = std::env::var_os(fleet_core_config::ENV_DATA_DIR).is_some();
    let database = match &options.database {
        Some(path) => path.clone(),
        None if config_existed || env_database || env_data_dir => loaded.storage.database.clone(),
        None => paths.database.clone(),
    };
    let database = expand_tilde(database);

    // The daemon config lives in the CLI config (or the documented default).
    // Loading it must not create directories or files.
    let cli_config = CliConfig::load(&options.cli_config_path)?;
    let daemon_config = cli_config
        .daemon_config
        .clone()
        .unwrap_or_else(default_daemon_config_path);
    let daemon = DaemonConfig::load(Some(&daemon_config))
        .with_context(|| format!("failed to load {}", daemon_config.display()))?;
    ensure_distinct_databases(&database, &expand_tilde(daemon.storage.database))?;

    // Bind address: explicit flag, otherwise the current value (or a prompt
    // defaulting to it). `prompt::input` silently uses the default when
    // prompting is disabled.
    let current_bind = format_bind(&loaded.server.host, loaded.server.port);
    let bind = match &options.bind {
        Some(bind) => bind.clone(),
        None => prompt::input(options.prompt, "Bind address (host:port)", &current_bind)?,
    };
    let (host, port) = parse_bind(&bind)?;

    // Web assets: `--no-web` wins; otherwise the explicit flag, an existing
    // static dir, or the role default. An empty interactive answer disables it.
    let web_dir = if options.no_web {
        None
    } else {
        let current = loaded
            .web
            .static_dir
            .clone()
            .or_else(|| paths.web_dir.clone());
        let current_text = current
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let chosen = match &options.web_dir {
            Some(path) => path.display().to_string(),
            None => prompt::input(
                options.prompt,
                "Web assets directory (empty to disable)",
                &current_text,
            )?,
        };
        if chosen.trim().is_empty() {
            None
        } else {
            Some(expand_tilde(PathBuf::from(chosen)))
        }
    };

    // Generate before any write so the config and env file share one token.
    let existing_token = loaded.auth.admin_token.trim().to_string();
    let (mut token, token_generated) = if existing_token.is_empty() || options.rotate_admin_token {
        (generate_admin_token(), true)
    } else {
        (existing_token, false)
    };
    if !token_generated
        && options.prompt.interactive()
        && prompt::confirm(options.prompt, "Generate a new admin token?", false)?
    {
        token = generate_admin_token();
    }

    let write_cli_config = if !options.write_cli_config {
        false
    } else if options.prompt.interactive() {
        prompt::confirm(options.prompt, "Write the CLI configuration?", true)?
    } else {
        true
    };

    let install_service = match options.install_service {
        Some(value) => value,
        None if options.prompt.interactive() => prompt::confirm(
            options.prompt,
            "Install the systemd service?",
            systemd_available(),
        )?,
        None => systemd_available(),
    };

    let admin_url = admin_url(&host, port);

    Ok(AdminSetupPlan {
        host,
        port,
        database,
        web_dir,
        token,
        token_generated,
        write_cli_config,
        install_service,
        daemon_config,
        admin_url,
    })
}

/// Configure the admin service, then optionally write the CLI config and
/// install the unit.
pub async fn run(options: AdminSetupOptions) -> Result<()> {
    let plan = build_plan(&options)?;

    if options.prompt.dry_run {
        print_dry_run(&plan, &options);
        if plan.install_service {
            // `systemd::install` prints the unit and commands without writing
            // or executing anything when `dry_run` is set.
            systemd::install(
                &options.paths,
                Some(&plan.token),
                true,
                true,
                options.prompt.yes,
            )?;
        }
        return Ok(());
    }

    // User-scope state directories are owned by the current account. System
    // scope is created by the privileged install commands.
    if !options.paths.is_system() {
        create_private_dir(&options.paths.state_dir)?;
    }

    write_admin_config(&options.paths, &plan)?;

    if plan.write_cli_config {
        write_cli_config(&options.cli_config_path, &options.paths, &plan)?;
    }

    if plan.install_service {
        let outcome = systemd::install(
            &options.paths,
            Some(&plan.token),
            true,
            false,
            options.prompt.yes,
        )?;
        if !outcome.skipped {
            if wait_for_health(&plan.admin_url).await {
                println!("admin is healthy at {}", plan.admin_url);
            } else {
                println!(
                    "admin did not become healthy; check `systemctl status {}` and `fleet admin logs`",
                    options.paths.role.unit_name()
                );
            }
        }
    } else if options.start {
        println!("starting the admin with the fallback supervisor");
        crate::process::start_role(&options.paths, crate::cli::ProcessMode::Auto, 15).await?;
    }

    Ok(())
}

fn write_admin_config(paths: &RolePaths, plan: &AdminSetupPlan) -> Result<()> {
    let mut edits = vec![
        TomlEdit::string("server", "host", plan.host.clone()),
        TomlEdit::integer("server", "port", i64::from(plan.port)),
        TomlEdit::string("storage", "database", plan.database.display().to_string()),
        TomlEdit::string("auth", "admin_token", plan.token.clone()),
    ];
    if let Some(web_dir) = &plan.web_dir {
        edits.push(TomlEdit::string(
            "web",
            "static_dir",
            web_dir.display().to_string(),
        ));
    }
    let removals: &[(&str, &str)] = if plan.web_dir.is_none() {
        &[("web", "static_dir")]
    } else {
        &[]
    };

    let backup = edit_toml_with_removals(&paths.config, &edits, removals)?;

    // Fail loudly if a future edit would produce a config the server rejects.
    let raw = fs::read_to_string(&paths.config)
        .with_context(|| format!("failed to read {}", paths.config.display()))?;
    AdminConfig::from_str(&raw)
        .with_context(|| format!("invalid admin config {}", paths.config.display()))?;

    println!(
        "wrote {}{}",
        paths.config.display(),
        backup
            .map(|path| format!(" (backup: {})", path.display()))
            .unwrap_or_default()
    );
    Ok(())
}

fn write_cli_config(
    path: &std::path::Path,
    paths: &RolePaths,
    plan: &AdminSetupPlan,
) -> Result<()> {
    let existed = path.exists();
    let mut cli = CliConfig::load(path)?;
    if existed {
        let backup = backup_file(path)?;
        println!("backed up {} to {}", path.display(), backup.display());
    }
    cli.admin_url = Some(plan.admin_url.clone());
    cli.token = Some(plan.token.clone());
    cli.admin_config = Some(paths.config.clone());
    cli.daemon_config = Some(plan.daemon_config.clone());
    cli.save(path)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn print_dry_run(plan: &AdminSetupPlan, options: &AdminSetupOptions) {
    println!("dry run; nothing will be written");
    println!("admin config  {}", options.paths.config.display());
    println!("bind          {}", format_bind(&plan.host, plan.port));
    println!("database      {}", plan.database.display());
    match &plan.web_dir {
        Some(web_dir) => println!("web dir       {}", web_dir.display()),
        None => println!("web dir       disabled"),
    }
    println!(
        "admin token   {}",
        if plan.token_generated {
            "generate"
        } else {
            "reuse"
        }
    );
    println!(
        "cli config    {}",
        if plan.write_cli_config {
            options.cli_config_path.display().to_string()
        } else {
            "skip".to_string()
        }
    );
    println!(
        "service       {}",
        if plan.install_service {
            "install"
        } else {
            "skip"
        }
    );
}

async fn wait_for_health(admin_url: &str) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(client) = AdminClient::new(admin_url, None) {
            if client.health().await.is_ok() {
                return true;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn parse_bind(bind: &str) -> Result<(String, u16)> {
    let bind = bind.trim();
    if bind.is_empty() {
        bail!("bind address must be host:port");
    }
    let (host, port) = if let Some(rest) = bind.strip_prefix('[') {
        let (host, rest) = rest
            .split_once(']')
            .ok_or_else(|| anyhow::anyhow!("invalid IPv6 bind address `{bind}`"))?;
        let port = rest
            .strip_prefix(':')
            .ok_or_else(|| anyhow::anyhow!("bind address must be host:port: `{bind}`"))?;
        (host.to_string(), port)
    } else {
        let (host, port) = bind
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("bind address must be host:port: `{bind}`"))?;
        (host.to_string(), port)
    };
    if host.is_empty() {
        bail!("bind address must include a host: `{bind}`");
    }
    let port: u16 = port
        .parse()
        .with_context(|| format!("invalid port in bind address `{bind}`"))?;
    Ok((host, port))
}

fn format_bind(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Loopback URL used for the post-install health probe.
fn admin_url(host: &str, port: u16) -> String {
    let host = match host {
        "0.0.0.0" => "127.0.0.1",
        "::" | "[::]" => "::1",
        other => other,
    };
    if host.contains(':') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)] // tests serialize process-wide env mutations
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;
    use crate::config_paths::{Role, Scope};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_VARS: &[&str] = &[
        "FLEET_DB",
        "FLEET_DATA_DIR",
        "FLEET_ADMIN_TOKEN",
        "FLEET_BIND",
        "FLEET_WEB_DIR",
        "FLEET_CONFIG",
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
            role: Role::Admin,
            scope: Scope::User,
            config: dir.join("admin/config.toml"),
            state_dir: dir.join("admin/state"),
            database: dir.join("admin/state/fleet-admin.db"),
            binary: None,
            fleet_binary: PathBuf::from("/usr/local/bin/fleet"),
            service_user: "tester".to_string(),
            service_group: "tester".to_string(),
            env_file: None,
            web_dir: None,
        }
    }

    fn options(dir: &Path, paths: RolePaths) -> AdminSetupOptions {
        AdminSetupOptions {
            paths,
            bind: None,
            database: None,
            web_dir: None,
            no_web: false,
            rotate_admin_token: false,
            write_cli_config: true,
            install_service: Some(false),
            start: false,
            prompt: PromptOptions {
                yes: true,
                dry_run: false,
            },
            cli_config_path: dir.join("cli.toml"),
        }
    }

    /// Seed the CLI config with a temp daemon config so planning never falls
    /// back to the developer's home directory.
    fn seed_cli_config(dir: &Path) -> PathBuf {
        let cli_path = dir.join("cli.toml");
        let daemon_path = dir.join("daemon/config.toml");
        fs::write(
            &cli_path,
            format!("daemon_config = \"{}\"\n", daemon_path.display()),
        )
        .unwrap();
        cli_path
    }

    #[tokio::test]
    async fn run_generates_fla_token_and_expected_endpoints() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let cli_path = seed_cli_config(dir.path());
        let paths = test_paths(dir.path());
        let config_path = paths.config.clone();
        let database = dir.path().join("data/custom.db");

        let mut options = options(dir.path(), paths);
        options.bind = Some("127.0.0.1:9099".to_string());
        options.database = Some(database.clone());
        run(options).await.unwrap();

        let raw = fs::read_to_string(&config_path).unwrap();
        let config = AdminConfig::from_str(&raw).unwrap();
        assert!(config.auth.admin_token.starts_with("fla_"));
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9099);
        assert_eq!(config.storage.database, database);

        let cli = CliConfig::load(&cli_path).unwrap();
        assert_eq!(cli.admin_url.as_deref(), Some("http://127.0.0.1:9099"));
        assert_eq!(cli.token.as_deref(), Some(config.auth.admin_token.as_str()));
        assert_eq!(cli.admin_config.as_deref(), Some(config_path.as_path()));
        assert!(cli.daemon_config.is_some());
    }

    #[test]
    fn rejects_database_collision_with_daemon_config() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let daemon_path = dir.path().join("daemon.toml");
        let shared = dir.path().join("shared.db");
        fs::write(
            &daemon_path,
            format!("[storage]\ndatabase = \"{}\"\n", shared.display()),
        )
        .unwrap();
        let cli_path = dir.path().join("cli.toml");
        fs::write(
            &cli_path,
            format!("daemon_config = \"{}\"\n", daemon_path.display()),
        )
        .unwrap();

        let mut options = options(dir.path(), test_paths(dir.path()));
        options.cli_config_path = cli_path;
        options.database = Some(shared);

        // `AdminSetupPlan` intentionally does not implement `Debug` because it
        // carries the admin token.
        let error = match build_plan(&options) {
            Ok(_) => panic!("colliding databases must be rejected"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("must be different"), "{error}");
    }

    #[tokio::test]
    async fn dry_run_writes_nothing() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let cli_path = seed_cli_config(dir.path());
        let before = fs::read_to_string(&cli_path).unwrap();

        let mut options = options(dir.path(), test_paths(dir.path()));
        options.bind = Some("127.0.0.1:9999".to_string());
        options.database = Some(dir.path().join("custom.db"));
        options.prompt.dry_run = true;
        run(options).await.unwrap();

        assert!(!dir.path().join("admin/config.toml").exists());
        assert!(!dir.path().join("admin/state").exists());
        assert_eq!(fs::read_to_string(&cli_path).unwrap(), before);
    }

    #[tokio::test]
    async fn no_web_removes_static_dir_from_existing_config() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        seed_cli_config(dir.path());
        let paths = test_paths(dir.path());
        fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        let static_dir = dir.path().join("web");
        fs::write(
            &paths.config,
            format!(
                "[server]\nhost = \"127.0.0.1\"\nport = 8787\n\n[web]\nstatic_dir = \"{}\"\n",
                static_dir.display()
            ),
        )
        .unwrap();

        let mut options = options(dir.path(), paths);
        options.no_web = true;
        options.database = Some(dir.path().join("custom.db"));
        run(options).await.unwrap();

        let raw = fs::read_to_string(dir.path().join("admin/config.toml")).unwrap();
        assert!(!raw.contains("static_dir"), "{raw}");
        assert!(raw.contains("[server]"), "{raw}");
    }
}

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::client::AdminClient;
use crate::config::{
    backup_file, default_admin_config_path, default_cli_config_path, default_daemon_config_path,
    CliConfig, DEFAULT_ADMIN_URL,
};
use crate::config_paths::{is_root, Role, RolePathOverrides, RolePaths, ScopeRequest};
use crate::connect::{self, ConnectOptions};
use crate::network::{self, NetworkMode, WizardOptions};
use crate::output;
use crate::prompt::{self, PromptOptions};

#[derive(Debug, Parser)]
#[command(
    name = "fleet",
    version,
    about = "Fleet command line: manage the admin, daemons, and remote access"
)]
pub struct Cli {
    /// Fleet admin base URL (default: http://127.0.0.1:8787).
    #[arg(long, global = true, env = "FLEET_ADMIN_URL")]
    pub admin_url: Option<String>,

    /// Admin bearer token (default: FLEET_ADMIN_TOKEN or cli.toml).
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

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect or create the CLI configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Admin control-plane commands.
    Admin {
        #[command(flatten)]
        role: RoleArgs,
        #[command(subcommand)]
        action: AdminCommand,
    },
    /// Daemon/worker commands.
    Daemon {
        #[command(flatten)]
        role: RoleArgs,
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

#[derive(Debug, Clone, Default, Args)]
pub struct ServiceArgs {
    #[arg(long)]
    pub yes: bool,
    #[arg(long)]
    pub dry_run: bool,
}

/// Role-scoped path and scope flags shared by the `admin` and `daemon`
/// namespaces.
#[derive(Debug, Clone, Default, Args)]
pub struct RoleArgs {
    /// Role config TOML (default: ~/.fleet/<role>/config.toml).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Role state directory (default: ~/.fleet/<role>).
    #[arg(long, global = true)]
    pub state_dir: Option<PathBuf>,
    /// Server binary (default: FLEET_<ROLE>_BIN, next to fleet, then PATH).
    #[arg(long, global = true)]
    pub bin: Option<PathBuf>,
    /// Force the system install scope.
    #[arg(long, global = true)]
    pub system: bool,
    /// Force the user install scope.
    #[arg(long, global = true)]
    pub user: bool,
}

impl RoleArgs {
    pub fn scope_request(&self) -> Result<ScopeRequest> {
        match (self.system, self.user) {
            (true, true) => bail!("--system and --user are mutually exclusive"),
            (true, false) => Ok(ScopeRequest::System),
            (false, true) => Ok(ScopeRequest::User),
            (false, false) => Ok(ScopeRequest::Auto),
        }
    }

    pub fn paths(&self, role: Role, database: Option<PathBuf>) -> Result<RolePaths> {
        RolePaths::resolve(
            role,
            self.scope_request()?,
            &RolePathOverrides {
                config: self.config.clone(),
                state_dir: self.state_dir.clone(),
                database,
                bin: self.bin.clone(),
            },
        )
    }

    /// Resolve paths for a setup workflow, prompting for the scope when the
    /// user did not choose one explicitly.
    pub fn setup_paths(
        &self,
        role: Role,
        database: Option<PathBuf>,
        prompt: PromptOptions,
    ) -> Result<RolePaths> {
        let request = self.scope_request()?;
        let request = if request == ScopeRequest::Auto && prompt.interactive() {
            let root = is_root();
            let default = usize::from(root);
            let choice = crate::prompt::select(
                prompt,
                "Install scope",
                &[
                    "user (current account)",
                    "system (root, background services)",
                ],
                default,
            )?;
            if choice == 1 {
                ScopeRequest::System
            } else {
                ScopeRequest::User
            }
        } else {
            request
        };
        let paths = RolePaths::resolve(
            role,
            request,
            &RolePathOverrides {
                config: self.config.clone(),
                state_dir: self.state_dir.clone(),
                database,
                bin: self.bin.clone(),
            },
        )?;
        if paths.is_system() && !is_root() {
            bail!("system scope requires root; rerun with sudo or pass --user");
        }
        Ok(paths)
    }
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Write a CLI configuration file.
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
    /// Show the resolved configuration (token redacted by default).
    Show {
        #[arg(long)]
        reveal: bool,
    },
    /// Validate the configuration and probe the admin.
    Validate,
    /// Print the default configuration path.
    Path,
}

#[derive(Debug, Subcommand)]
pub enum NetworkCommand {
    /// Report installed tooling, tunnel state and admin reachability.
    Detect,
    /// Interactively generate admin/daemon/tunnel configuration.
    Wizard {
        #[arg(long, value_enum)]
        mode: Option<ModeArg>,
        /// Public hostname; defaults to the Tailscale DNS name.
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        admin_config: Option<PathBuf>,
        #[arg(long)]
        daemon_config: Option<PathBuf>,
        #[arg(long)]
        cloudflared_config: Option<PathBuf>,
        /// Apply without interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Authorize and automatically connect Tailscale or Cloudflare.
    Connect {
        #[arg(long, value_enum)]
        provider: ModeArg,
        /// Public hostname; defaults to the Tailscale DNS name.
        #[arg(long)]
        hostname: Option<String>,
        /// Run the vendor login, tunnel, service and verification steps.
        /// Without this flag only a preview is printed.
        #[arg(long)]
        execute: bool,
        /// Confirm every step without prompting.
        #[arg(long)]
        yes: bool,
        /// Admin TOML to update (default: ~/.fleet/admin/config.toml).
        #[arg(long)]
        admin_config: Option<PathBuf>,
        /// Daemon TOML to update (default: ~/.fleet/daemon/config.toml).
        #[arg(long)]
        daemon_config: Option<PathBuf>,
        /// Cloudflare ingress file (default: ~/.fleet/cloudflared.yml).
        #[arg(long)]
        cloudflared_config: Option<PathBuf>,
        /// Scoped token used only to create the Cloudflare Access app/policy.
        #[arg(long, env = "CLOUDFLARE_API_TOKEN")]
        cloudflare_api_token: Option<String>,
        /// Cloudflare account id (discovered from the token when omitted).
        #[arg(long)]
        cloudflare_account_id: Option<String>,
        /// Allow exactly this email through Cloudflare Access.
        #[arg(long)]
        access_email: Option<String>,
        /// Allow this email domain through Cloudflare Access.
        #[arg(long)]
        access_domain: Option<String>,
        /// Allow everyone through Cloudflare Access (combine with another control).
        #[arg(long)]
        access_everyone: bool,
        /// Do not create a Cloudflare Access application.
        #[arg(long)]
        skip_access: bool,
        /// Do not install cloudflared as a system service.
        #[arg(long)]
        skip_service: bool,
        /// Offer to install a missing vendor tool with Homebrew (macOS).
        #[arg(long)]
        install: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ModeArg {
    Tailscale,
    Cloudflare,
}

impl From<ModeArg> for NetworkMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Tailscale => NetworkMode::Tailscale,
            ModeArg::Cloudflare => NetworkMode::Cloudflare,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum MachinesCommand {
    List,
    Get {
        id: String,
    },
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    Reconnect {
        id: String,
    },
    Providers {
        id: String,
    },
    Revoked,
    Unrevoke {
        id: String,
    },
    MoveOrg {
        id: String,
        org_id: String,
    },
    PairCode {
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkspacesCommand {
    List {
        #[arg(long)]
        machine: Option<String>,
    },
    Get {
        id: String,
    },
    Add {
        #[arg(long)]
        machine: String,
        #[arg(long)]
        root: String,
        #[arg(long)]
        name: Option<String>,
    },
    Remove {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    List,
    Get {
        id: String,
    },
    Create {
        #[arg(long)]
        machine: String,
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    Send {
        id: String,
        message: String,
    },
    Permissions {
        id: String,
    },
    Interrupt {
        id: String,
    },
    Resume {
        id: String,
    },
    Close {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum UsersCommand {
    List {
        #[arg(long)]
        org: Option<String>,
    },
    Create {
        #[arg(long)]
        org: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = ["admin", "member", "viewer"])]
        role: String,
    },
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    RotateToken {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrgsCommand {
    List,
    Create {
        #[arg(long)]
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum PermissionsCommand {
    Approve { id: String },
    Reject { id: String },
}

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
        if let Some(client) = self.client.get() {
            return Ok(client);
        }
        let client = AdminClient::new(&self.admin_url, self.token.clone())?;
        let _ = self.client.set(client);
        self.client
            .get()
            .ok_or_else(|| anyhow::anyhow!("failed to initialize admin client"))
    }

    fn require_token(&self) -> Result<()> {
        if self.token.is_some() {
            return Ok(());
        }
        bail!(
            "an admin token is required; pass --admin-token, set FLEET_ADMIN_TOKEN, or run `fleet config init`"
        )
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli
        .cli_config
        .clone()
        .unwrap_or_else(default_cli_config_path);
    let config = CliConfig::load(&config_path)?;
    let admin_url = cli
        .admin_url
        .clone()
        .or_else(|| config.admin_url.clone())
        .unwrap_or_else(|| DEFAULT_ADMIN_URL.to_string());
    let token = cli.admin_token.clone().or_else(|| config.token.clone());
    let json = cli.json || config.output.as_deref() == Some("json");
    let ctx = AppContext {
        token,
        json,
        config,
        config_path,
        admin_url,
        client: std::sync::OnceLock::new(),
    };

    match cli.command {
        Command::Config { action } => config_command(&ctx, action).await,
        Command::Admin { role, action } => admin_command(&ctx, role, action).await,
        Command::Daemon { role, action } => daemon_command(&ctx, role, action).await,
        Command::Network { action } => network_command(&ctx, action).await,
        Command::Setup => {
            bail!(
                "`fleet setup` is implemented in Plan 2; use `fleet admin setup` or `fleet daemon setup`"
            )
        }
        Command::Status => {
            bail!("`fleet status` is implemented in Plan 4")
        }
        Command::Doctor => {
            bail!("`fleet doctor` is implemented in Plan 4")
        }
    }
}

async fn admin_command(ctx: &AppContext, role: RoleArgs, action: AdminCommand) -> Result<()> {
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
        AdminCommand::Run => crate::process::run_role(&role.paths(Role::Admin, None)?),
        AdminCommand::InstallService(args) => {
            let paths = role.paths(Role::Admin, None)?;
            let token = admin_token_from_config(&paths);
            crate::process::systemd::install(
                &paths,
                token.as_deref(),
                true,
                args.dry_run,
                args.yes,
            )?;
            Ok(())
        }
        AdminCommand::UninstallService(args) => {
            let paths = role.paths(Role::Admin, None)?;
            crate::process::systemd::uninstall(&paths, args.dry_run, args.yes)
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

async fn daemon_command(ctx: &AppContext, role: RoleArgs, action: DaemonCommand) -> Result<()> {
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
        DaemonCommand::Run => crate::process::run_role(&role.paths(Role::Daemon, None)?),
        DaemonCommand::InstallService(args) => {
            let paths = role.paths(Role::Daemon, None)?;
            crate::process::systemd::install(&paths, None, true, args.dry_run, args.yes)?;
            Ok(())
        }
        DaemonCommand::UninstallService(args) => {
            let paths = role.paths(Role::Daemon, None)?;
            crate::process::systemd::uninstall(&paths, args.dry_run, args.yes)
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

/// Load the admin bearer token for the service env file. If the config cannot
/// be read (or does not define a token), the transient environment token is
/// left to the caller's environment.
fn admin_token_from_config(paths: &RolePaths) -> Option<String> {
    fleet_core_config::AdminConfig::load(Some(&paths.config))
        .ok()
        .map(|config| config.auth.admin_token)
        .filter(|token| !token.trim().is_empty())
}

async fn health(ctx: &AppContext) -> Result<()> {
    let value = ctx.client()?.health().await?;
    if ctx.json {
        return output::print_json(&value);
    }
    output::print_fields(
        &value,
        &[
            ("ok", "ok"),
            ("version", "version"),
            ("machines_online", "machines online"),
        ],
    );
    Ok(())
}

async fn config_command(ctx: &AppContext, action: ConfigCommand) -> Result<()> {
    match action {
        ConfigCommand::Init {
            admin_url,
            admin_token,
            output: format,
            force,
        } => {
            if ctx.config_path.exists() {
                if !force {
                    bail!(
                        "{} already exists; pass --force to overwrite",
                        ctx.config_path.display()
                    );
                }
                let backup = backup_file(&ctx.config_path)?;
                println!(
                    "backed up {} to {}",
                    ctx.config_path.display(),
                    backup.display()
                );
            }
            let config = CliConfig {
                admin_url: Some(admin_url.unwrap_or_else(|| ctx.admin_url.clone())),
                token: admin_token.or_else(|| ctx.token.clone()),
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
            config.save(&ctx.config_path)?;
            println!("wrote {}", ctx.config_path.display());
            if config.token.is_none() {
                println!(
                    "no token stored; pass --admin-token or export FLEET_ADMIN_TOKEN when needed"
                );
            }
            Ok(())
        }
        ConfigCommand::Show { reveal } => {
            println!("path   {}", ctx.config_path.display());
            println!("url    {}", ctx.admin_url);
            println!(
                "token  {}",
                match &ctx.token {
                    Some(token) if reveal => token.as_str(),
                    Some(_) => "<redacted>",
                    None => "<none>",
                }
            );
            if let Some(public_url) = &ctx.config.public_url {
                println!("public {public_url}");
            }
            Ok(())
        }
        ConfigCommand::Validate => {
            config_project(&ctx.config_path)?;
            println!("config {} is valid TOML", ctx.config_path.display());
            println!("url    {}", ctx.admin_url);
            let health = ctx.client()?.health().await;
            match health {
                Ok(value) => println!(
                    "admin  reachable (version {})",
                    value
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                ),
                Err(error) => {
                    println!("admin  unreachable: {error}");
                    bail!("admin health probe failed: {error}");
                }
            }
            Ok(())
        }
        ConfigCommand::Path => {
            println!("{}", ctx.config_path.display());
            Ok(())
        }
    }
}

fn config_project(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    raw.parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("invalid TOML in {}", path.display()))?;
    Ok(())
}

async fn network_command(ctx: &AppContext, action: NetworkCommand) -> Result<()> {
    match action {
        NetworkCommand::Detect => {
            let report = network::detect(&ctx.admin_url, ctx.token.clone()).await;
            if ctx.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                network::print_report(&report);
            }
            Ok(())
        }
        NetworkCommand::Wizard {
            mode,
            hostname,
            admin_config,
            daemon_config,
            cloudflared_config,
            yes,
        } => {
            let mut options = WizardOptions::new(ctx.admin_url.clone(), ctx.token.clone());
            options.mode = mode.map(Into::into);
            options.hostname = hostname;
            options.admin_config = admin_config
                .or_else(|| ctx.config.admin_config.clone())
                .unwrap_or_else(default_admin_config_path);
            options.daemon_config = daemon_config
                .or_else(|| ctx.config.daemon_config.clone())
                .unwrap_or_else(default_daemon_config_path);
            options.cli_config = ctx.config_path.clone();
            if let Some(path) = cloudflared_config {
                options.cloudflared_config = path;
            }
            options.yes = yes;
            network::wizard(options).await
        }
        NetworkCommand::Connect {
            provider,
            hostname,
            execute,
            yes,
            admin_config,
            daemon_config,
            cloudflared_config,
            cloudflare_api_token,
            cloudflare_account_id,
            access_email,
            access_domain,
            access_everyone,
            skip_access,
            skip_service,
            install,
        } => {
            let admin_config = admin_config
                .or_else(|| ctx.config.admin_config.clone())
                .unwrap_or_else(default_admin_config_path);
            let daemon_config = daemon_config
                .or_else(|| ctx.config.daemon_config.clone())
                .unwrap_or_else(default_daemon_config_path);
            let cloudflared_config = cloudflared_config.unwrap_or_else(|| {
                crate::config::home_dir()
                    .join(".fleet")
                    .join("cloudflared.yml")
            });
            connect::connect(ConnectOptions {
                provider: provider.into(),
                hostname,
                admin_url: ctx.admin_url.clone(),
                admin_config,
                daemon_config,
                cli_config: ctx.config_path.clone(),
                cloudflared_config,
                execute,
                yes,
                cloudflare_api_token,
                cloudflare_account_id,
                access_email,
                access_domain,
                access_everyone,
                skip_access,
                skip_service,
                install,
            })
            .await
        }
    }
}

async fn machines(ctx: &AppContext, action: MachinesCommand) -> Result<()> {
    match action {
        MachinesCommand::List => {
            let value = with_token(ctx, ctx.client()?.get("/api/v1/machines")).await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "ID"),
                    ("name", "NAME"),
                    ("status", "STATUS"),
                    ("hostname", "HOST"),
                    ("workspace_count", "WS"),
                    ("daemon_version", "DAEMON"),
                ],
            )
        }
        MachinesCommand::Get { id } => {
            let value =
                with_token(ctx, ctx.client()?.get(&format!("/api/v1/machines/{id}"))).await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "id"),
                    ("name", "name"),
                    ("status", "status"),
                    ("hostname", "hostname"),
                    ("os", "os"),
                    ("arch", "arch"),
                    ("daemon_version", "daemon"),
                    ("workspace_count", "workspaces"),
                    ("last_seen_at", "last seen"),
                ],
            )
        }
        MachinesCommand::Delete { id, yes } => {
            ctx.require_token()?;
            confirm(&format!("Delete machine {id}?"), yes)?;
            ctx.client()?
                .delete(&format!("/api/v1/machines/{id}"))
                .await?;
            println!("revoked machine {id}; unrevoke with `fleet admin machines unrevoke {id}`");
            Ok(())
        }
        MachinesCommand::Revoked => {
            let value = with_token(ctx, ctx.client()?.get("/api/v1/machines/revoked")).await?;
            emit(
                ctx,
                &value,
                &[
                    ("machine_id", "MACHINE"),
                    ("revoked_at", "REVOKED"),
                    ("reason", "REASON"),
                ],
            )
        }
        MachinesCommand::Unrevoke { id } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/machines/{id}/unrevoke"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("machine {id} unrevoked"))
        }
        MachinesCommand::Reconnect { id } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/machines/{id}/reconnect"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("reconnect requested for {id}"))
        }
        MachinesCommand::Providers { id } => {
            let value = with_token(
                ctx,
                ctx.client()?
                    .get(&format!("/api/v1/machines/{id}/providers")),
            )
            .await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "ID"),
                    ("display_name", "NAME"),
                    ("available", "AVAILABLE"),
                    ("version", "VERSION"),
                ],
            )
        }
        MachinesCommand::MoveOrg { id, org_id } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?.put(
                    &format!("/api/v1/machines/{id}/org"),
                    json!({"org_id": org_id}),
                ),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("machine {id} moved to {org_id}"))
        }
        MachinesCommand::PairCode { org, name } => {
            require_token(ctx)?;
            let client = ctx.client()?;
            crate::pairing::create_code(client, org, name, ctx.json).await
        }
    }
}

async fn workspaces(ctx: &AppContext, action: WorkspacesCommand) -> Result<()> {
    match action {
        WorkspacesCommand::List { machine } => {
            let path = match machine {
                Some(machine) => format!("/api/v1/workspaces?machine_id={machine}"),
                None => "/api/v1/workspaces".to_string(),
            };
            let value = with_token(ctx, ctx.client()?.get(&path)).await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "ID"),
                    ("name", "NAME"),
                    ("machine_id", "MACHINE"),
                    ("root_path", "ROOT"),
                    ("is_git_repository", "GIT"),
                ],
            )
        }
        WorkspacesCommand::Get { id } => {
            let value =
                with_token(ctx, ctx.client()?.get(&format!("/api/v1/workspaces/{id}"))).await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "id"),
                    ("name", "name"),
                    ("machine_id", "machine"),
                    ("root_path", "root"),
                    ("is_git_repository", "git"),
                ],
            )
        }
        WorkspacesCommand::Add {
            machine,
            root,
            name,
        } => {
            require_token(ctx)?;
            let mut body = json!({"machine_id": machine, "root_path": root});
            if let Some(name) = name {
                body["name"] = Value::String(name);
            }
            let value = with_token(ctx, ctx.client()?.post("/api/v1/workspaces", body)).await?;
            emit_json_or(ctx, &value, "workspace registered")
        }
        WorkspacesCommand::Remove { id, yes } => {
            require_token(ctx)?;
            confirm(&format!("Remove workspace {id}?"), yes)?;
            ctx.client()?
                .delete(&format!("/api/v1/workspaces/{id}"))
                .await?;
            println!("removed workspace {id}");
            Ok(())
        }
    }
}

async fn sessions(ctx: &AppContext, action: SessionsCommand) -> Result<()> {
    match action {
        SessionsCommand::List => {
            let value = with_token(ctx, ctx.client()?.get("/api/v1/sessions")).await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "ID"),
                    ("title", "TITLE"),
                    ("status", "STATUS"),
                    ("provider", "PROVIDER"),
                    ("workspace_id", "WORKSPACE"),
                    ("updated_at", "UPDATED"),
                ],
            )
        }
        SessionsCommand::Get { id } => {
            let value =
                with_token(ctx, ctx.client()?.get(&format!("/api/v1/sessions/{id}"))).await?;
            if ctx.json {
                return output::print_json(&value);
            }
            let session = value.get("session").unwrap_or(&value);
            output::print_fields(
                session,
                &[
                    ("id", "id"),
                    ("title", "title"),
                    ("status", "status"),
                    ("provider", "provider"),
                    ("machine_id", "machine"),
                    ("workspace_id", "workspace"),
                    ("cwd", "cwd"),
                    ("created_at", "created"),
                    ("updated_at", "updated"),
                ],
            );
            Ok(())
        }
        SessionsCommand::Create {
            machine,
            workspace,
            provider,
            title,
            model,
        } => {
            require_token(ctx)?;
            let mut body = json!({"machine_id": machine, "workspace_id": workspace});
            if let Some(provider) = provider {
                body["provider"] = Value::String(provider);
            }
            if let Some(title) = title {
                body["title"] = Value::String(title);
            }
            if let Some(model) = model {
                body["model"] = Value::String(model);
            }
            let value = with_token(ctx, ctx.client()?.post("/api/v1/sessions", body)).await?;
            emit_json_or(ctx, &value, "session created")
        }
        SessionsCommand::Send { id, message } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?.post(
                    &format!("/api/v1/sessions/{id}/messages"),
                    json!({"content": message}),
                ),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("message sent to {id}"))
        }
        SessionsCommand::Permissions { id } => {
            let value = with_token(
                ctx,
                ctx.client()?
                    .get(&format!("/api/v1/sessions/{id}/permissions")),
            )
            .await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "ID"),
                    ("tool_name", "TOOL"),
                    ("status", "STATUS"),
                    ("created_at", "CREATED"),
                ],
            )
        }
        SessionsCommand::Interrupt { id } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/sessions/{id}/interrupt"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("interrupted {id}"))
        }
        SessionsCommand::Resume { id } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/sessions/{id}/resume"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("resumed {id}"))
        }
        SessionsCommand::Close { id, yes } => {
            require_token(ctx)?;
            confirm(&format!("Close and archive session {id}?"), yes)?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/sessions/{id}/close"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("closed session {id}"))
        }
        SessionsCommand::Delete { id, yes } => {
            require_token(ctx)?;
            confirm(
                &format!("Permanently delete session {id} and its history?"),
                yes,
            )?;
            ctx.client()?
                .delete(&format!("/api/v1/sessions/{id}"))
                .await?;
            println!("deleted session {id}");
            Ok(())
        }
    }
}

async fn users(ctx: &AppContext, action: UsersCommand) -> Result<()> {
    match action {
        UsersCommand::List { org } => {
            let path = match org {
                Some(org) => format!("/api/v1/users?org_id={org}"),
                None => "/api/v1/users".to_string(),
            };
            let value = with_token(ctx, ctx.client()?.get(&path)).await?;
            emit(
                ctx,
                &value,
                &[
                    ("id", "ID"),
                    ("name", "NAME"),
                    ("org_id", "ORG"),
                    ("role", "ROLE"),
                    ("created_at", "CREATED"),
                ],
            )
        }
        UsersCommand::Create { org, name, role } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?.post(
                    "/api/v1/users",
                    json!({"org_id": org, "name": name, "role": role}),
                ),
            )
            .await?;
            if ctx.json {
                return output::print_json(&value);
            }
            output::print_fields(
                &value,
                &[
                    ("user.id", "user"),
                    ("user.name", "name"),
                    ("user.org_id", "org"),
                    ("user.role", "role"),
                    ("token", "token"),
                ],
            );
            println!("\nstore this token now; it will not be shown again.");
            Ok(())
        }
        UsersCommand::Delete { id, yes } => {
            require_token(ctx)?;
            confirm(&format!("Delete user {id}?"), yes)?;
            ctx.client()?.delete(&format!("/api/v1/users/{id}")).await?;
            println!("deleted user {id}");
            Ok(())
        }
        UsersCommand::RotateToken { id, yes } => {
            require_token(ctx)?;
            confirm(
                &format!(
                    "Rotate the admin token for user {id}? The previous token will stop working."
                ),
                yes,
            )?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/users/{id}/rotate-token"), json!({})),
            )
            .await?;
            if ctx.json {
                return output::print_json(&value);
            }
            output::print_fields(&value, &[("user_id", "user"), ("token", "token")]);
            println!("\nstore this token now; the previous token is invalid.");
            Ok(())
        }
    }
}

async fn orgs(ctx: &AppContext, action: OrgsCommand) -> Result<()> {
    match action {
        OrgsCommand::List => {
            let value = with_token(ctx, ctx.client()?.get("/api/v1/orgs")).await?;
            emit(
                ctx,
                &value,
                &[("id", "ID"), ("name", "NAME"), ("created_at", "CREATED")],
            )
        }
        OrgsCommand::Create { name } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?.post("/api/v1/orgs", json!({"name": name})),
            )
            .await?;
            emit_json_or(ctx, &value, "organization created")
        }
    }
}

async fn audit(ctx: &AppContext, limit: usize) -> Result<()> {
    let value = with_token(
        ctx,
        ctx.client()?.get(&format!("/api/v1/audit?limit={limit}")),
    )
    .await?;
    if ctx.json {
        return output::print_json(&value);
    }
    let entries = value
        .get("entries")
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    emit(
        ctx,
        &entries,
        &[
            ("created_at", "TIME"),
            ("actor", "ACTOR"),
            ("action", "ACTION"),
            ("subject", "SUBJECT"),
            ("details.outcome", "OUTCOME"),
        ],
    )
}

async fn permissions(ctx: &AppContext, action: PermissionsCommand) -> Result<()> {
    require_token(ctx)?;
    let (path, done) = match action {
        PermissionsCommand::Approve { id } => (
            format!("/api/v1/permissions/{id}/approve"),
            format!("approved {id}"),
        ),
        PermissionsCommand::Reject { id } => (
            format!("/api/v1/permissions/{id}/reject"),
            format!("rejected {id}"),
        ),
    };
    let value = with_token(ctx, ctx.client()?.post(&path, json!({}))).await?;
    emit_json_or(ctx, &value, &done)
}

fn emit(ctx: &AppContext, value: &Value, columns: &[(&str, &str)]) -> Result<()> {
    if ctx.json {
        output::print_json(value)
    } else {
        output::print_rows(value, columns);
        Ok(())
    }
}

fn emit_json_or(ctx: &AppContext, value: &Value, message: &str) -> Result<()> {
    if ctx.json {
        output::print_json(value)
    } else {
        println!("{message}");
        Ok(())
    }
}

fn require_token(ctx: &AppContext) -> Result<()> {
    ctx.require_token()
}

async fn with_token(
    ctx: &AppContext,
    future: impl std::future::Future<Output = Result<Value>>,
) -> Result<Value> {
    ctx.require_token()?;
    future.await
}

fn confirm(message: &str, yes: bool) -> Result<()> {
    let options = PromptOptions {
        yes,
        dry_run: false,
    };
    if prompt::confirm(options, message, false)? {
        Ok(())
    } else {
        bail!("aborted")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("parse")
    }

    #[test]
    fn parses_machine_list_and_global_flags() {
        let cli = parse(&[
            "fleet",
            "--json",
            "--admin-url",
            "http://example:8787",
            "admin",
            "machines",
            "list",
        ]);
        assert!(cli.json);
        assert_eq!(cli.admin_url.as_deref(), Some("http://example:8787"));
        assert!(matches!(
            cli.command,
            Command::Admin {
                role: _,
                action: AdminCommand::Machines {
                    action: MachinesCommand::List
                }
            }
        ));
    }

    #[test]
    fn parses_network_wizard() {
        let cli = parse(&[
            "fleet",
            "network",
            "wizard",
            "--mode",
            "tailscale",
            "--hostname",
            "fleet.example.com",
            "--yes",
        ]);
        match cli.command {
            Command::Network {
                action:
                    NetworkCommand::Wizard {
                        mode,
                        hostname,
                        yes,
                        ..
                    },
            } => {
                assert!(matches!(mode, Some(ModeArg::Tailscale)));
                assert_eq!(hostname.as_deref(), Some("fleet.example.com"));
                assert!(yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_network_connect_flags() {
        let cli = parse(&[
            "fleet",
            "network",
            "connect",
            "--provider",
            "cloudflare",
            "--hostname",
            "fleet.example.com",
            "--execute",
            "--yes",
            "--skip-access",
            "--access-email",
            "ada@example.com",
        ]);
        match cli.command {
            Command::Network {
                action:
                    NetworkCommand::Connect {
                        provider,
                        hostname,
                        execute,
                        yes,
                        skip_access,
                        access_email,
                        ..
                    },
            } => {
                assert!(matches!(provider, ModeArg::Cloudflare));
                assert_eq!(hostname.as_deref(), Some("fleet.example.com"));
                assert!(execute);
                assert!(yes);
                assert!(skip_access);
                assert_eq!(access_email.as_deref(), Some("ada@example.com"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_users_rotate_token_confirmation_flag() {
        let cli = parse(&["fleet", "admin", "users", "rotate-token", "user_1", "--yes"]);
        match cli.command {
            Command::Admin {
                role: _,
                action:
                    AdminCommand::Users {
                        action: UsersCommand::RotateToken { id, yes },
                    },
            } => {
                assert_eq!(id, "user_1");
                assert!(yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = parse(&["fleet", "admin", "users", "rotate-token", "user_1"]);
        match cli.command {
            Command::Admin {
                role: _,
                action:
                    AdminCommand::Users {
                        action: UsersCommand::RotateToken { yes, .. },
                    },
            } => assert!(!yes),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    fn test_context(config_path: std::path::PathBuf, admin_url: &str) -> AppContext {
        AppContext {
            token: None,
            json: false,
            config: CliConfig::default(),
            config_path,
            admin_url: admin_url.to_string(),
            client: std::sync::OnceLock::new(),
        }
    }

    #[test]
    fn admin_client_is_lazy() {
        let ctx = AppContext {
            token: None,
            json: false,
            config: CliConfig::default(),
            config_path: PathBuf::from("/tmp/fleet-cli.toml"),
            admin_url: "not-a-url".to_string(),
            client: std::sync::OnceLock::new(),
        };
        assert!(ctx.client().is_err());
    }

    #[tokio::test]
    async fn config_init_force_backs_up_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli.toml");
        let original = "admin_url = \"http://old.example\"\n";
        std::fs::write(&path, original).unwrap();

        let ctx = test_context(path.clone(), DEFAULT_ADMIN_URL);
        config_command(
            &ctx,
            ConfigCommand::Init {
                admin_url: Some("http://new.example:8787".into()),
                admin_token: Some("secret".into()),
                output: Some("json".into()),
                force: true,
            },
        )
        .await
        .expect("config init");

        let backup = dir.path().join("cli.toml.bak");
        assert!(backup.exists(), "existing config should be backed up");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);

        let config = CliConfig::load(&path).unwrap();
        assert_eq!(config.admin_url.as_deref(), Some("http://new.example:8787"));
        assert_eq!(config.token.as_deref(), Some("secret"));
    }

    #[tokio::test]
    async fn config_validate_fails_when_health_probe_fails() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path().join("cli.toml"), "http://127.0.0.1:1");
        let result = config_command(&ctx, ConfigCommand::Validate).await;
        assert!(
            result.is_err(),
            "an unreachable admin must make config validate exit non-zero"
        );
    }

    #[test]
    fn rejects_unknown_roles() {
        let error = Cli::try_parse_from([
            "fleet",
            "admin",
            "users",
            "create",
            "--org",
            "org_default",
            "--name",
            "Ada",
            "--role",
            "owner",
        ])
        .expect_err("invalid role");
        assert!(error.to_string().contains("invalid value"));
    }

    #[test]
    fn old_top_level_machine_command_is_rejected() {
        assert!(Cli::try_parse_from(["fleet", "machines", "list"]).is_err());
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn parses_admin_machines() {
        let cli = Cli::try_parse_from(["fleet", "admin", "machines", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Admin {
                role: _,
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
                role: _,
                action: DaemonCommand::Setup { .. }
            }
        ));
    }

    #[test]
    fn parses_top_level_config_and_network() {
        assert!(matches!(
            Cli::try_parse_from(["fleet", "config", "path"])
                .unwrap()
                .command,
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

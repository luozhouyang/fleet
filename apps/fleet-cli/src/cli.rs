use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::client::AdminClient;
use crate::config::{
    backup_file, default_admin_config_path, default_cli_config_path, default_daemon_config_path,
    CliConfig, DEFAULT_ADMIN_URL,
};
use crate::config_paths::{is_root, Role, RolePathOverrides, RolePaths};
use crate::connect::{self, ConnectOptions};
use crate::network::{self, NetworkMode, WizardOptions};
use crate::output;
use crate::prompt::{self, PromptOptions};
use crate::setup::{self, SetupRole};

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
    /// Inspect agents available on this machine.
    Agents {
        #[command(subcommand)]
        action: AgentsCommand,
    },
    /// Inspect registered agent drivers.
    Drivers {
        #[command(subcommand)]
        action: DriversCommand,
    },
    /// Detect and configure Tailscale / Cloudflare remote access.
    Network {
        #[command(subcommand)]
        action: NetworkCommand,
    },
    /// Interactive setup dispatcher.
    Setup {
        /// Role to set up; prompts when omitted.
        #[arg(long, value_enum)]
        role: Option<SetupRoleArg>,
        /// Accept the defaults without prompting.
        #[arg(long)]
        yes: bool,
        /// Arguments forwarded to the role-specific setup command.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Install Fleet as a server-profile service.
    Install(crate::install::InstallArgs),
    /// Summarize admin and daemon status.
    Status,
    /// Diagnose configuration and process state.
    Doctor,
    /// Upgrade the installed binaries from a release.
    Upgrade(UpgradeArgs),
    /// Remove services, binaries, and optionally data.
    Uninstall(UninstallArgs),
    /// Internal: run the detached fallback supervisor.
    #[command(name = "__supervise", hide = true)]
    Supervise {
        #[arg(value_enum)]
        role: SetupRoleArg,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        state_dir: PathBuf,
        #[arg(long)]
        bin: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SetupRoleArg {
    Admin,
    Daemon,
    Cli,
}

impl From<SetupRoleArg> for SetupRole {
    fn from(value: SetupRoleArg) -> Self {
        match value {
            SetupRoleArg::Admin => SetupRole::Admin,
            SetupRoleArg::Daemon => SetupRole::Daemon,
            SetupRoleArg::Cli => SetupRole::Cli,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Set up the admin service.
    Setup(AdminSetupArgs),
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
    /// Install the platform service (systemd, launchd, or Windows service).
    InstallService(ServiceArgs),
    /// Remove the platform service.
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
    Setup(crate::setup::daemon::DaemonSetupArgs),
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

/// Agent catalog commands.
#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    /// List agent definitions, their driver and availability.
    List,
    /// Inspect one agent definition.
    Inspect {
        /// Agent definition id.
        id: String,
    },
    /// Probe one agent without creating a session.
    Probe {
        /// Agent definition id.
        id: String,
    },
}

/// Driver catalog commands.
#[derive(Debug, Subcommand)]
pub enum DriversCommand {
    /// List registered drivers.
    List,
    /// Inspect one driver.
    Inspect {
        /// Driver id.
        id: String,
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
    /// Use the platform service manager (systemd, launchd, or Windows services).
    Service,
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
    /// Show the supervisor log instead of the server log.
    #[arg(long)]
    pub supervisor: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct ServiceArgs {
    #[arg(long)]
    pub yes: bool,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args)]
pub struct UpgradeArgs {
    /// Release tag to install (default: latest).
    #[arg(long, default_value = "latest")]
    pub version: String,
    /// Reinstall even when the version matches.
    #[arg(long)]
    pub force: bool,
    /// Replace binaries without restarting installed services.
    #[arg(long)]
    pub no_restart: bool,
    /// Do not prompt for confirmation.
    #[arg(long)]
    pub yes: bool,
    /// Print the plan without changing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args)]
pub struct UninstallArgs {
    #[command(flatten)]
    pub role: RoleArgs,
    /// Keep configuration, databases, keys, and secrets (the default).
    #[arg(long, conflicts_with = "delete_data")]
    pub keep_data: bool,
    /// Also delete configuration and data.
    #[arg(long = "delete-data", alias = "purge")]
    pub delete_data: bool,
    /// Do not prompt for confirmation.
    #[arg(long)]
    pub yes: bool,
    /// Print the plan without changing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Configuration written by `fleet admin setup`. Role-scoped flags such as
/// `--config`, `--state-dir`, and `--bin` come from [`RoleArgs`].
#[derive(Debug, Clone, Args)]
pub struct AdminSetupArgs {
    /// Admin listener bind address (host:port).
    #[arg(long)]
    pub bind: Option<String>,
    /// SQLite database path (default: the role's state directory).
    #[arg(long)]
    pub database: Option<PathBuf>,
    /// Built web UI directory to serve.
    #[arg(long, conflicts_with = "no_web")]
    pub web_dir: Option<PathBuf>,
    /// Do not serve the web UI.
    #[arg(long)]
    pub no_web: bool,
    /// Generate a new admin token even when one already exists.
    #[arg(long)]
    pub rotate_admin_token: bool,
    /// Do not write the CLI configuration.
    #[arg(long)]
    pub no_cli_config: bool,
    /// Install the platform service (systemd, launchd, or Windows service).
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

/// Role-scoped path overrides shared by the `admin` and `daemon` namespaces.
///
/// Fleet has a single server profile: configuration and state always live under
/// `~/.fleet`, and services always run as the invoking user.
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
}

impl RoleArgs {
    fn overrides(&self, database: Option<PathBuf>) -> RolePathOverrides {
        RolePathOverrides {
            config: self.config.clone(),
            state_dir: self.state_dir.clone(),
            database,
            bin: self.bin.clone(),
        }
    }

    pub fn paths(&self, role: Role, database: Option<PathBuf>) -> Result<RolePaths> {
        RolePaths::resolve(role, &self.overrides(database))
    }

    /// Resolve configuration paths without discovering the server binary.
    pub fn config_paths(&self, role: Role, database: Option<PathBuf>) -> Result<RolePaths> {
        RolePaths::resolve_config(role, &self.overrides(database))
    }

    /// Resolve paths for a setup workflow.
    pub fn setup_paths(
        &self,
        role: Role,
        database: Option<PathBuf>,
        _prompt: PromptOptions,
    ) -> Result<RolePaths> {
        self.paths(role, database)
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
        /// Organization the new account joins.
        #[arg(long)]
        org: String,
        /// Verified email of the new account; invitations and identity
        /// linking match on it.
        #[arg(long)]
        email: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = ["owner", "admin", "member", "viewer"])]
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
    /// Block a user's token and revoke their browser sessions.
    Disable {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Restore a disabled account.
    Enable {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrgsCommand {
    /// Organizations the current account belongs to.
    List,
    Create {
        #[arg(long)]
        name: String,
    },
    /// Members of one organization.
    Members {
        #[arg(long)]
        org: String,
    },
    /// Invite an email address that has no account yet.
    Invite {
        #[arg(long)]
        org: String,
        #[arg(long)]
        email: String,
        #[arg(long, value_parser = ["owner", "admin", "member", "viewer"])]
        role: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum PermissionsCommand {
    Approve { session: String, id: String },
    Reject { session: String, id: String },
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
    if is_root() {
        bail!(
            "do not run fleet as root; run it as your normal user. Fleet keeps data in your home and asks for elevation only for fixed service-definition commands"
        );
    }
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
        Command::Install(args) => crate::install::run(args).await,
        Command::Config { action } => config_command(&ctx, action).await,
        Command::Admin { role, action } => admin_command(&ctx, role, action).await,
        Command::Daemon { role, action } => daemon_command(&ctx, role, action).await,
        Command::Agents { action } => {
            let paths = top_level_paths(&ctx, Role::Daemon)?;
            let args: Vec<String> = match action {
                AgentsCommand::List => vec!["agents".to_string()],
                AgentsCommand::Inspect { id } => {
                    vec!["agents".to_string(), "--id".to_string(), id]
                }
                AgentsCommand::Probe { id } => vec!["probe".to_string(), id],
            };
            crate::local::run_daemon(&args, &paths).await
        }
        Command::Drivers { action } => {
            let paths = top_level_paths(&ctx, Role::Daemon)?;
            let args: Vec<String> = match action {
                DriversCommand::List => vec!["drivers".to_string()],
                DriversCommand::Inspect { id } => {
                    vec!["drivers".to_string(), "--id".to_string(), id]
                }
            };
            crate::local::run_daemon(&args, &paths).await
        }
        Command::Network { action } => network_command(&ctx, action).await,
        Command::Setup { role, yes, args } => {
            setup::dispatch(role.map(Into::into), yes, args).await
        }
        Command::Status => {
            let admin = top_level_paths(&ctx, Role::Admin)?;
            let daemon = top_level_paths(&ctx, Role::Daemon)?;
            let code = crate::fleet_status::run(&admin, &daemon, ctx.json).await?;
            std::process::exit(code);
        }
        Command::Doctor => {
            let admin = top_level_paths(&ctx, Role::Admin)?;
            let daemon = top_level_paths(&ctx, Role::Daemon)?;
            let code = crate::doctor::run(&admin, &daemon, ctx.json).await?;
            std::process::exit(code);
        }
        Command::Upgrade(args) => {
            crate::upgrade::run(crate::upgrade::UpgradeOptions {
                version: args.version,
                force: args.force,
                no_restart: args.no_restart,
                yes: args.yes,
                dry_run: args.dry_run,
            })
            .await
        }
        Command::Uninstall(args) => {
            crate::uninstall::run(crate::uninstall::UninstallOptions {
                overrides: RolePathOverrides {
                    config: args.role.config.clone(),
                    state_dir: args.role.state_dir.clone(),
                    bin: args.role.bin.clone(),
                    database: None,
                },
                delete_data: args.delete_data && !args.keep_data,
                yes: args.yes,
                dry_run: args.dry_run,
            })
            .await
        }
        Command::Supervise {
            role,
            config,
            state_dir,
            bin,
        } => {
            let role = match role {
                SetupRoleArg::Admin => Role::Admin,
                SetupRoleArg::Daemon => Role::Daemon,
                SetupRoleArg::Cli => bail!("__supervise requires admin or daemon"),
            };
            crate::process::supervisor::supervise(role, config, state_dir, bin).await
        }
    }
}

/// Resolve both role paths for top-level `fleet status` and `fleet doctor`.
/// The CLI config may record custom config paths written by setup.
fn top_level_paths(ctx: &AppContext, role: Role) -> Result<RolePaths> {
    let config = match role {
        Role::Admin => ctx.config.admin_config.clone(),
        Role::Daemon => ctx.config.daemon_config.clone(),
    };
    RolePaths::resolve_config(
        role,
        &RolePathOverrides {
            config,
            ..RolePathOverrides::default()
        },
    )
}

async fn admin_command(ctx: &AppContext, role: RoleArgs, action: AdminCommand) -> Result<()> {
    match action {
        AdminCommand::Health => health(ctx).await,
        AdminCommand::Setup(args) => {
            let prompt = PromptOptions {
                yes: args.yes,
                dry_run: args.dry_run,
            };
            let paths = role.setup_paths(Role::Admin, args.database.clone(), prompt)?;
            let install_service = if args.install_service {
                Some(true)
            } else if args.no_install_service {
                Some(false)
            } else {
                None
            };
            crate::setup::admin::run(crate::setup::admin::AdminSetupOptions {
                paths,
                bind: args.bind,
                database: args.database,
                web_dir: args.web_dir,
                no_web: args.no_web,
                rotate_admin_token: args.rotate_admin_token,
                write_cli_config: !args.no_cli_config,
                install_service,
                start: args.start,
                prompt,
                cli_config_path: ctx.config_path.clone(),
            })
            .await
        }
        AdminCommand::Config { action } => {
            let paths = role.config_paths(Role::Admin, None)?;
            let daemon_config = ctx
                .config
                .daemon_config
                .clone()
                .unwrap_or_else(default_daemon_config_path);
            match action {
                AdminConfigCommand::Show { reveal } => {
                    crate::config_commands::admin_show(&paths, reveal, ctx.json)
                }
                AdminConfigCommand::Path => crate::config_commands::admin_path(&paths),
                AdminConfigCommand::Validate => {
                    crate::config_commands::admin_validate(&paths, &daemon_config)
                }
            }
        }
        AdminCommand::Start(args) => {
            crate::process::start_role(&role.paths(Role::Admin, None)?, args.mode, args.timeout)
                .await
        }
        AdminCommand::Stop(args) => {
            crate::process::stop_role(
                &role.config_paths(Role::Admin, None)?,
                args.mode,
                args.timeout,
            )
            .await
        }
        AdminCommand::Restart(args) => {
            crate::process::restart_role(&role.paths(Role::Admin, None)?, args.mode, args.timeout)
                .await
        }
        AdminCommand::Status(args) => {
            let paths = role.config_paths(Role::Admin, None)?;
            let code = crate::process::status::status(&paths, args.mode, ctx.json).await?;
            std::process::exit(code);
        }
        AdminCommand::Logs(args) => {
            let paths = role.config_paths(Role::Admin, None)?;
            crate::process::status::logs(
                &paths,
                args.mode,
                args.lines,
                args.follow,
                args.supervisor,
            )
            .await
        }
        AdminCommand::Run => crate::process::run_role(&role.paths(Role::Admin, None)?).await,
        AdminCommand::InstallService(args) => {
            let paths = role.paths(Role::Admin, None)?;
            let token = admin_token_from_config(&paths);
            crate::process::service::install(
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
            crate::process::service::uninstall(&paths, args.dry_run, args.yes)
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
        DaemonCommand::Setup(args) => {
            let prompt = PromptOptions {
                yes: args.yes,
                dry_run: args.dry_run,
            };
            let paths = role.setup_paths(Role::Daemon, args.database.clone(), prompt)?;
            crate::setup::daemon::run(crate::setup::daemon::DaemonSetupOptions {
                paths,
                args,
                admin_url: ctx.admin_url.clone(),
                admin_token: ctx.token.clone(),
                prompt,
                cli_config_path: ctx.config_path.clone(),
            })
            .await
        }
        DaemonCommand::Config { action } => {
            let paths = role.config_paths(Role::Daemon, None)?;
            let admin_config = ctx
                .config
                .admin_config
                .clone()
                .unwrap_or_else(default_admin_config_path);
            match action {
                DaemonConfigCommand::Show => crate::config_commands::daemon_show(&paths, ctx.json),
                DaemonConfigCommand::Path => crate::config_commands::daemon_path(&paths),
                DaemonConfigCommand::Validate => {
                    crate::config_commands::daemon_validate(&paths, &admin_config)
                }
            }
        }
        DaemonCommand::Start(args) => {
            crate::process::start_role(&role.paths(Role::Daemon, None)?, args.mode, args.timeout)
                .await
        }
        DaemonCommand::Stop(args) => {
            crate::process::stop_role(
                &role.config_paths(Role::Daemon, None)?,
                args.mode,
                args.timeout,
            )
            .await
        }
        DaemonCommand::Restart(args) => {
            crate::process::restart_role(&role.paths(Role::Daemon, None)?, args.mode, args.timeout)
                .await
        }
        DaemonCommand::Status(args) => {
            let paths = role.config_paths(Role::Daemon, None)?;
            let code = crate::process::status::status(&paths, args.mode, ctx.json).await?;
            std::process::exit(code);
        }
        DaemonCommand::Logs(args) => {
            let paths = role.config_paths(Role::Daemon, None)?;
            crate::process::status::logs(
                &paths,
                args.mode,
                args.lines,
                args.follow,
                args.supervisor,
            )
            .await
        }
        DaemonCommand::Run => crate::process::run_role(&role.paths(Role::Daemon, None)?).await,
        DaemonCommand::InstallService(args) => {
            let paths = role.paths(Role::Daemon, None)?;
            crate::process::service::install(&paths, None, true, args.dry_run, args.yes)?;
            Ok(())
        }
        DaemonCommand::UninstallService(args) => {
            let paths = role.paths(Role::Daemon, None)?;
            crate::process::service::uninstall(&paths, args.dry_run, args.yes)
        }
        DaemonCommand::Pair { server, code, name } => {
            let paths = role.config_paths(Role::Daemon, None)?;
            crate::pairing::pair_only(&paths.config, server, code, name).await
        }
        DaemonCommand::Providers => {
            let paths = role.paths(Role::Daemon, None)?;
            crate::local::run_daemon(&["providers".to_string()], &paths).await
        }
        DaemonCommand::Workspace { action } => {
            let paths = role.paths(Role::Daemon, None)?;
            let args = match action {
                DaemonWorkspaceCommand::Add { root, name } => {
                    let mut args =
                        vec!["register-workspace".to_string(), root.display().to_string()];
                    if let Some(name) = name {
                        args.push("--name".to_string());
                        args.push(name);
                    }
                    args
                }
                DaemonWorkspaceCommand::List => vec!["list-workspaces".to_string()],
                DaemonWorkspaceCommand::Remove { id, yes } => {
                    let mut args = vec!["remove-workspace".to_string(), id];
                    if yes {
                        args.push("--yes".to_string());
                    }
                    args
                }
            };
            crate::local::run_daemon(&args, &paths).await
        }
        DaemonCommand::RotateKey => {
            let paths = role.paths(Role::Daemon, None)?;
            crate::local::run_daemon(&["rotate-key".to_string()], &paths).await
        }
        DaemonCommand::Identity { action } => match action {
            IdentityCommand::Show => {
                let paths = role.config_paths(Role::Daemon, None)?;
                let value = crate::local::identity_json(&paths)?;
                println!("{}", serde_json::to_string_pretty(&value)?);
                Ok(())
            }
        },
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
                    ("email", "EMAIL"),
                    ("name", "NAME"),
                    ("created_at", "CREATED"),
                ],
            )
        }
        UsersCommand::Create {
            org,
            email,
            name,
            role,
        } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?.post(
                    "/api/v1/users",
                    json!({"org_id": org, "email": email, "name": name, "role": role}),
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
                    ("user.email", "email"),
                    ("user.name", "name"),
                    ("membership.org_id", "org"),
                    ("membership.role", "role"),
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
        UsersCommand::Disable { id, yes } => {
            require_token(ctx)?;
            confirm(
                &format!("Disable user {id}? Their token and browser sessions stop working."),
                yes,
            )?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/users/{id}/disable"), json!({})),
            )
            .await?;
            if ctx.json {
                return output::print_json(&value);
            }
            output::print_fields(
                &value,
                &[
                    ("id", "USER"),
                    ("email", "EMAIL"),
                    ("name", "NAME"),
                    ("disabled_at", "DISABLED"),
                ],
            );
            Ok(())
        }
        UsersCommand::Enable { id, yes } => {
            require_token(ctx)?;
            confirm(&format!("Enable user {id}?"), yes)?;
            let value = with_token(
                ctx,
                ctx.client()?
                    .post(&format!("/api/v1/users/{id}/enable"), json!({})),
            )
            .await?;
            if ctx.json {
                return output::print_json(&value);
            }
            output::print_fields(
                &value,
                &[
                    ("id", "USER"),
                    ("email", "EMAIL"),
                    ("name", "NAME"),
                    ("disabled_at", "DISABLED"),
                ],
            );
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
                &[
                    ("organization.id", "ID"),
                    ("organization.name", "NAME"),
                    ("organization.kind", "KIND"),
                    ("role", "ROLE"),
                ],
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
        OrgsCommand::Members { org } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?.get(&format!("/api/v1/orgs/{org}/members")),
            )
            .await?;
            emit(
                ctx,
                &value,
                &[
                    ("user_id", "USER"),
                    ("email", "EMAIL"),
                    ("name", "NAME"),
                    ("role", "ROLE"),
                ],
            )
        }
        OrgsCommand::Invite { org, email, role } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client()?.post(
                    &format!("/api/v1/orgs/{org}/invitations"),
                    json!({"email": email, "role": role}),
                ),
            )
            .await?;
            if ctx.json {
                return output::print_json(&value);
            }
            output::print_fields(
                &value,
                &[
                    ("invitation.org_id", "org"),
                    ("invitation.email", "email"),
                    ("invitation.role", "role"),
                    ("token", "token"),
                ],
            );
            println!("\nshare this one-time token with the invitee.");
            Ok(())
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
        PermissionsCommand::Approve { session, id } => (
            format!("/api/v1/sessions/{session}/permissions/{id}/approve"),
            format!("approved {id}"),
        ),
        PermissionsCommand::Reject { session, id } => (
            format!("/api/v1/sessions/{session}/permissions/{id}/reject"),
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
                action: DaemonCommand::Setup(_)
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
            Command::Setup {
                role: None,
                yes: false,
                args: _
            }
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

    #[test]
    fn parses_upgrade_options() {
        let cli = Cli::try_parse_from([
            "fleet",
            "upgrade",
            "--version",
            "v0.2.0",
            "--force",
            "--no-restart",
            "--yes",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Command::Upgrade(args) => {
                assert_eq!(args.version, "v0.2.0");
                assert!(args.force);
                assert!(args.no_restart);
                assert!(args.yes);
                assert!(args.dry_run);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_uninstall_data_choice() {
        let cli = Cli::try_parse_from(["fleet", "uninstall", "--delete-data", "--yes"]).unwrap();
        match cli.command {
            Command::Uninstall(args) => {
                assert!(args.delete_data);
                assert!(!args.keep_data);
                assert!(args.yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from(["fleet", "uninstall", "--purge"]).unwrap();
        match cli.command {
            Command::Uninstall(args) => {
                assert!(args.delete_data);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        assert!(
            Cli::try_parse_from(["fleet", "uninstall", "--keep-data", "--delete-data",]).is_err()
        );
    }

    #[test]
    fn parses_setup_role_yes_and_forwarded_args() {
        match Cli::try_parse_from([
            "fleet",
            "setup",
            "--role",
            "daemon",
            "--yes",
            "--state-dir",
            "/tmp/fleet",
            "--name",
            "box",
        ])
        .unwrap()
        .command
        {
            Command::Setup { role, yes, args } => {
                assert!(matches!(role, Some(SetupRoleArg::Daemon)));
                assert!(yes);
                assert_eq!(
                    args,
                    strings(&["--state-dir", "/tmp/fleet", "--name", "box"])
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }
}

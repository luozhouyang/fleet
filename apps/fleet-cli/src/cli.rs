use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::client::AdminClient;
use crate::config::{
    backup_file, default_cli_config_path, default_service_config_path, CliConfig, DEFAULT_ADMIN_URL,
};
use crate::connect::{self, ConnectOptions};
use crate::network::{self, NetworkMode, WizardOptions};
use crate::output;

#[derive(Debug, Parser)]
#[command(
    name = "af",
    version,
    about = "Agent Fleet command line: manage the admin API and remote access"
)]
pub struct Cli {
    /// Agent admin base URL (default: http://127.0.0.1:8787).
    #[arg(long, global = true, env = "AGENT_FLEET_ADMIN_URL")]
    pub url: Option<String>,

    /// Bearer token (default: AGENT_FLEET_ADMIN_TOKEN or cli.toml).
    #[arg(long, global = true, env = "AGENT_FLEET_ADMIN_TOKEN")]
    pub token: Option<String>,

    /// CLI configuration file (default: ~/.agent-fleet/cli.toml).
    #[arg(long, global = true, env = "AGENT_FLEET_CLI_CONFIG")]
    pub config: Option<PathBuf>,

    /// Print raw JSON instead of tables.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that the admin is reachable.
    Health,
    /// Inspect or create the CLI configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Detect and configure Tailscale / Cloudflare remote access.
    Network {
        #[command(subcommand)]
        action: NetworkCommand,
    },
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
        /// Maximum entries (server default 100, max 500).
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Approve or reject a pending permission.
    Permissions {
        #[command(subcommand)]
        action: PermissionsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Write a CLI configuration file.
    Init {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        token: Option<String>,
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
        /// Machine token written to the daemon connection config.
        #[arg(long, env = "AGENT_FLEET_MACHINE_TOKEN")]
        machine_token: Option<String>,
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
        /// Machine token written to the daemon connection config.
        #[arg(long, env = "AGENT_FLEET_MACHINE_TOKEN")]
        machine_token: Option<String>,
        /// Admin TOML to update (default: ~/.agent-fleet/config.toml).
        #[arg(long)]
        admin_config: Option<PathBuf>,
        /// Daemon TOML to update (default: ~/.agent-fleet/config.toml).
        #[arg(long)]
        daemon_config: Option<PathBuf>,
        /// Cloudflare ingress file (default: ~/.agent-fleet/cloudflared.yml).
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
    client: AdminClient,
    token: Option<String>,
    json: bool,
    config: CliConfig,
    config_path: PathBuf,
    admin_url: String,
}

impl AppContext {
    fn require_token(&self) -> Result<()> {
        if self.token.is_some() {
            return Ok(());
        }
        bail!(
            "an admin token is required; pass --token, set AGENT_FLEET_ADMIN_TOKEN, or run `af config init`"
        )
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(default_cli_config_path);
    let config = CliConfig::load(&config_path)?;
    let admin_url = cli
        .url
        .clone()
        .or_else(|| config.admin_url.clone())
        .unwrap_or_else(|| DEFAULT_ADMIN_URL.to_string());
    let token = cli.token.clone().or_else(|| config.token.clone());
    let json = cli.json || config.output.as_deref() == Some("json");
    let client = AdminClient::new(&admin_url, token.clone())?;
    let ctx = AppContext {
        client,
        token,
        json,
        config,
        config_path,
        admin_url,
    };

    match cli.command {
        Command::Health => health(&ctx).await,
        Command::Config { action } => config_command(&ctx, action).await,
        Command::Network { action } => network_command(&ctx, action).await,
        Command::Machines { action } => machines(&ctx, action).await,
        Command::Workspaces { action } => workspaces(&ctx, action).await,
        Command::Sessions { action } => sessions(&ctx, action).await,
        Command::Users { action } => users(&ctx, action).await,
        Command::Orgs { action } => orgs(&ctx, action).await,
        Command::Audit { limit } => audit(&ctx, limit).await,
        Command::Permissions { action } => permissions(&ctx, action).await,
    }
}

async fn health(ctx: &AppContext) -> Result<()> {
    let value = ctx.client.health().await?;
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
            url,
            token,
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
                admin_url: Some(url.unwrap_or_else(|| ctx.admin_url.clone())),
                token: token.or_else(|| ctx.token.clone()),
                output: format.or(Some(if ctx.json { "json" } else { "table" }.to_string())),
                public_url: ctx.config.public_url.clone(),
            };
            config.save(&ctx.config_path)?;
            println!("wrote {}", ctx.config_path.display());
            if config.token.is_none() {
                println!(
                    "no token stored; pass --token or export AGENT_FLEET_ADMIN_TOKEN when needed"
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
            let health = ctx.client.health().await;
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
            machine_token,
            admin_config,
            daemon_config,
            cloudflared_config,
            yes,
        } => {
            let mut options = WizardOptions::new(ctx.admin_url.clone(), ctx.token.clone());
            options.mode = mode.map(Into::into);
            options.hostname = hostname;
            if machine_token.is_some() {
                options.machine_token = machine_token;
            }
            options.admin_config = admin_config.unwrap_or_else(default_service_config_path);
            options.daemon_config = daemon_config.unwrap_or_else(default_service_config_path);
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
            machine_token,
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
            let admin_config = admin_config.unwrap_or_else(default_service_config_path);
            let daemon_config = daemon_config.unwrap_or_else(default_service_config_path);
            let cloudflared_config = cloudflared_config.unwrap_or_else(|| {
                crate::config::home_dir()
                    .join(".agent-fleet")
                    .join("cloudflared.yml")
            });
            connect::connect(ConnectOptions {
                provider: provider.into(),
                hostname,
                admin_url: ctx.admin_url.clone(),
                machine_token: machine_token
                    .or_else(|| std::env::var("AGENT_FLEET_MACHINE_TOKEN").ok()),
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
            let value = with_token(ctx, ctx.client.get("/api/v1/machines")).await?;
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
            let value = with_token(ctx, ctx.client.get(&format!("/api/v1/machines/{id}"))).await?;
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
            ctx.client.delete(&format!("/api/v1/machines/{id}")).await?;
            println!("revoked machine {id}; unrevoke with `af machines unrevoke {id}`");
            Ok(())
        }
        MachinesCommand::Revoked => {
            let value = with_token(ctx, ctx.client.get("/api/v1/machines/revoked")).await?;
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
                ctx.client
                    .post(&format!("/api/v1/machines/{id}/unrevoke"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("machine {id} unrevoked"))
        }
        MachinesCommand::Reconnect { id } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client
                    .post(&format!("/api/v1/machines/{id}/reconnect"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("reconnect requested for {id}"))
        }
        MachinesCommand::Providers { id } => {
            let value = with_token(
                ctx,
                ctx.client.get(&format!("/api/v1/machines/{id}/providers")),
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
                ctx.client.put(
                    &format!("/api/v1/machines/{id}/org"),
                    json!({"org_id": org_id}),
                ),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("machine {id} moved to {org_id}"))
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
            let value = with_token(ctx, ctx.client.get(&path)).await?;
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
                with_token(ctx, ctx.client.get(&format!("/api/v1/workspaces/{id}"))).await?;
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
            let value = with_token(ctx, ctx.client.post("/api/v1/workspaces", body)).await?;
            emit_json_or(ctx, &value, "workspace registered")
        }
        WorkspacesCommand::Remove { id, yes } => {
            require_token(ctx)?;
            confirm(&format!("Remove workspace {id}?"), yes)?;
            ctx.client
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
            let value = with_token(ctx, ctx.client.get("/api/v1/sessions")).await?;
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
            let value = with_token(ctx, ctx.client.get(&format!("/api/v1/sessions/{id}"))).await?;
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
            let value = with_token(ctx, ctx.client.post("/api/v1/sessions", body)).await?;
            emit_json_or(ctx, &value, "session created")
        }
        SessionsCommand::Send { id, message } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client.post(
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
                ctx.client
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
                ctx.client
                    .post(&format!("/api/v1/sessions/{id}/interrupt"), json!({})),
            )
            .await?;
            emit_json_or(ctx, &value, &format!("interrupted {id}"))
        }
        SessionsCommand::Resume { id } => {
            require_token(ctx)?;
            let value = with_token(
                ctx,
                ctx.client
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
                ctx.client
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
            ctx.client.delete(&format!("/api/v1/sessions/{id}")).await?;
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
            let value = with_token(ctx, ctx.client.get(&path)).await?;
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
                ctx.client.post(
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
            ctx.client.delete(&format!("/api/v1/users/{id}")).await?;
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
                ctx.client
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
            let value = with_token(ctx, ctx.client.get("/api/v1/orgs")).await?;
            emit(
                ctx,
                &value,
                &[("id", "ID"), ("name", "NAME"), ("created_at", "CREATED")],
            )
        }
        OrgsCommand::Create { name } => {
            require_token(ctx)?;
            let value =
                with_token(ctx, ctx.client.post("/api/v1/orgs", json!({"name": name}))).await?;
            emit_json_or(ctx, &value, "organization created")
        }
    }
}

async fn audit(ctx: &AppContext, limit: usize) -> Result<()> {
    let value = with_token(ctx, ctx.client.get(&format!("/api/v1/audit?limit={limit}"))).await?;
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
    let value = with_token(ctx, ctx.client.post(&path, json!({}))).await?;
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
    if yes {
        return Ok(());
    }
    print!("{message} [y/N]: ");
    io::stdout().flush().context("failed to flush stdout")?;
    let mut line = String::new();
    let read = io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read stdin")?;
    if read == 0 {
        bail!("stdin closed; pass --yes to confirm non-interactively");
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => bail!("aborted"),
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
            "af",
            "--json",
            "--url",
            "http://example:8787",
            "machines",
            "list",
        ]);
        assert!(cli.json);
        assert_eq!(cli.url.as_deref(), Some("http://example:8787"));
        assert!(matches!(
            cli.command,
            Command::Machines {
                action: MachinesCommand::List
            }
        ));
    }

    #[test]
    fn parses_network_wizard() {
        let cli = parse(&[
            "af",
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
            "af",
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
        let cli = parse(&["af", "users", "rotate-token", "user_1", "--yes"]);
        match cli.command {
            Command::Users {
                action: UsersCommand::RotateToken { id, yes },
            } => {
                assert_eq!(id, "user_1");
                assert!(yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = parse(&["af", "users", "rotate-token", "user_1"]);
        match cli.command {
            Command::Users {
                action: UsersCommand::RotateToken { yes, .. },
            } => assert!(!yes),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    fn test_context(config_path: std::path::PathBuf, admin_url: &str) -> AppContext {
        AppContext {
            client: AdminClient::new(admin_url, None).expect("admin client"),
            token: None,
            json: false,
            config: CliConfig::default(),
            config_path,
            admin_url: admin_url.to_string(),
        }
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
                url: Some("http://new.example:8787".into()),
                token: Some("secret".into()),
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
            "af",
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
}

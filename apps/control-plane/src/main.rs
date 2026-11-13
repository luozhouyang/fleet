//! Agent Fleet control plane: REST API, machine gateway and browser events.

mod app;
mod error;
mod middleware;
mod routes;
mod services;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agent_fleet_config::ControlPlaneConfig;
use agent_fleet_storage::Database;
use clap::{Args, Parser, Subcommand};

use crate::services::event_service;
use crate::state::AppState;

#[derive(Debug, Parser)]
#[command(name = "agent-fleet-control-plane", version, about)]
struct Cli {
    /// Path to a TOML config file (default: ~/.agent-fleet/config.toml if present).
    #[arg(long, global = true, env = "AGENT_FLEET_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the control plane (default).
    Serve(ServeArgs),
}

/// CLI overrides applied after the configuration file and environment.
#[derive(Debug, Default, Args)]
struct ServeArgs {
    /// Listener address, e.g. `127.0.0.1:8787`.
    #[arg(long)]
    bind: Option<String>,
    /// SQLite database path (`:memory:` is accepted for development).
    #[arg(long)]
    db: Option<PathBuf>,
    /// Admin bearer token for the REST API and machine gateway.
    #[arg(long)]
    admin_token: Option<String>,
    /// Directory containing the built web UI.
    #[arg(long)]
    web_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    agent_fleet_common::init_tracing("agent_fleet_control_plane=info");

    match cli.command.unwrap_or(Command::Serve(ServeArgs::default())) {
        Command::Serve(args) => serve(cli.config, args).await,
    }
}

async fn serve(config_path: Option<PathBuf>, args: ServeArgs) -> anyhow::Result<()> {
    let mut config = match config_path.as_deref() {
        Some(path) => ControlPlaneConfig::load(Some(path))?,
        None => ControlPlaneConfig::load_default()?,
    };

    if let Some(bind) = args.bind.as_deref() {
        let (host, port) = parse_bind(bind)?;
        config.server.host = host;
        config.server.port = port;
    }
    if let Some(db) = args.db {
        config.storage.database = db;
    }
    if let Some(token) = args.admin_token {
        config.auth.admin_token = token;
    }
    if let Some(dir) = args.web_dir {
        config.web.static_dir = Some(dir);
    }
    config.resolve_paths();

    // Serve the built UI automatically when running from the repository root.
    if config.web.static_dir.is_none() {
        for candidate in ["apps/web/dist", "web/dist", "dist"] {
            let path = PathBuf::from(candidate);
            if path.join("index.html").is_file() {
                config.web.static_dir = Some(path);
                break;
            }
        }
    }

    if config.auth.admin_token.trim().is_empty() {
        anyhow::bail!(
            "admin token must not be empty; pass --admin-token or set AGENT_FLEET_ADMIN_TOKEN"
        );
    }

    let database = if config.storage.database.as_os_str() == ":memory:" {
        Database::connect_in_memory().await?
    } else {
        Database::connect(&config.storage.database).await?
    };
    let state = AppState::new(Arc::new(database), config.clone()).await?;

    // Consume machine gateway events for the lifetime of the process.
    let event_task = event_service::spawn(state.clone());
    let router = app::build_router(state);
    let address = resolve_address(&config.server.host, config.server.port).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(
        %address,
        database = %config.storage.database.display(),
        "control plane listening"
    );

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    event_task.abort();
    result?;
    Ok(())
}

/// Split a `host:port` override. IPv6 hosts may be bracketed.
fn parse_bind(value: &str) -> anyhow::Result<(String, u16)> {
    let value = value.trim();
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid bind address `{value}`; expected host:port"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        anyhow::bail!("invalid bind address `{value}`; host is empty");
    }
    let port: u16 = port
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid port in bind address `{value}`"))?;
    Ok((host.to_string(), port))
}

async fn resolve_address(host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let mut addresses = tokio::net::lookup_host((host, port)).await?;
    addresses
        .next()
        .ok_or_else(|| anyhow::anyhow!("could not resolve {host}:{port}"))
}

async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %err, "failed to listen for ctrl-c");
    }
    tracing::info!("shutdown signal received");
}

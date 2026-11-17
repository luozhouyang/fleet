//! Fleet admin server library.

mod app;
mod error;
mod middleware;
mod routes;
mod services;
mod state;
#[cfg(windows)]
pub mod windows_service;

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

    /// Run under the Windows service control manager.
    #[arg(long, global = true, hide = true)]
    pub windows_service: bool,

    /// Run as a Task Scheduler task on Windows (enables file logging).
    #[arg(long, global = true, hide = true)]
    pub windows_task: bool,

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
    let mut config = match config_path.as_deref() {
        Some(path) => AdminConfig::load(Some(path))?,
        None => AdminConfig::load_default()?,
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
        anyhow::bail!("admin token must not be empty; pass --admin-token or set FLEET_ADMIN_TOKEN");
    }
    if config.auth.pair_code_ttl_secs == 0 {
        anyhow::bail!("auth.pair_code_ttl_secs must be greater than zero");
    }
    if config.auth.pair_code_max_attempts == 0 {
        anyhow::bail!(
            "auth.pair_code_max_attempts must be greater than zero (zero would lock every code)"
        );
    }
    let database = if config.storage.database.as_os_str() == ":memory:" {
        AdminStorage::connect_in_memory().await?
    } else {
        AdminStorage::connect(&config.storage.database).await?
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
        "fleet admin listening"
    );

    let result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
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
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "failed to listen for ctrl-c");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = fleet_core_common::shutdown::wait() => {}
    }
    tracing::info!("shutdown signal received");
}

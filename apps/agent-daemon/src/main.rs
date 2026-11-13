//! Agent Fleet machine daemon.
//!
//! Runs on the user's machine, owns agent processes, workspaces and Git access,
//! and maintains a reverse WebSocket connection to the control plane.

mod app;
mod bootstrap;
mod error;
mod events;
mod handler;
mod services;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "agent-fleet-daemon", version, about)]
struct Cli {
    /// Path to a TOML config file (default: ~/.agent-fleet/config.toml if present).
    #[arg(long, global = true, env = "AGENT_FLEET_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    agent_fleet_common::init_tracing("agent_fleet_daemon=info,agent_fleet_agent_runtime=info");

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => app::serve(cli.config).await,
        Command::RegisterWorkspace { root, name } => {
            bootstrap::register_workspace(cli.config, root, name).await
        }
        Command::Providers => bootstrap::print_providers(cli.config).await,
        Command::RotateKey => bootstrap::rotate_key(cli.config).await,
    }
}

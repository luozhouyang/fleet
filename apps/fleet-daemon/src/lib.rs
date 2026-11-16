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
    /// List registered local workspaces as JSON.
    ListWorkspaces,
    /// Remove a registered local workspace.
    RemoveWorkspace {
        /// Workspace id.
        id: String,
        /// Confirm removal without prompting.
        #[arg(long)]
        yes: bool,
    },
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
        Command::ListWorkspaces => bootstrap::list_workspaces(cli.config).await,
        Command::RemoveWorkspace { id, yes } => {
            bootstrap::remove_workspace(cli.config, id, yes).await
        }
        Command::RotateKey => bootstrap::rotate_key(cli.config).await,
        Command::Pair { server, code, name } => {
            bootstrap::pair(cli.config, server, code, name).await
        }
    }
}

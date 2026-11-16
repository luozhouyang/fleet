//! `fleet-daemon` process entrypoint.

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fleet_core_common::init_tracing("fleet_daemon=info,fleet_daemon_runtime=info");
    let cli = fleet_daemon::Cli::parse();
    fleet_daemon::run(cli).await
}

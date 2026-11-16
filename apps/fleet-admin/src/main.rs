//! `fleet-admin` process entrypoint.

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fleet_core_common::init_tracing("fleet_admin=info");
    let cli = fleet_admin::Cli::parse();
    fleet_admin::run(cli).await
}

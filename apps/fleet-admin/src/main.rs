//! `fleet-admin` process entrypoint.

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = fleet_admin::Cli::parse();

    #[cfg(windows)]
    if cli.windows_service {
        let log_path = cli
            .config
            .as_deref()
            .and_then(|path| path.parent())
            .map(|dir| dir.join("logs").join("admin.log"))
            .unwrap_or_else(|| std::env::temp_dir().join("fleet-admin.log"));
        fleet_core_common::init_tracing_to_file("fleet_admin=info", &log_path)?;
        fleet_admin::windows_service::run()?;
        return Ok(());
    }

    fleet_core_common::init_tracing("fleet_admin=info");
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fleet_admin::run(cli))
}

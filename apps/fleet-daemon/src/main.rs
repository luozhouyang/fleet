//! `fleet-daemon` process entrypoint.

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = fleet_daemon::Cli::parse();

    #[cfg(windows)]
    if cli.windows_task {
        let log_path = cli
            .config
            .as_deref()
            .and_then(|path| path.parent())
            .map(|dir| dir.join("logs").join("daemon.log"))
            .unwrap_or_else(|| std::env::temp_dir().join("fleet-daemon.log"));
        fleet_core_common::init_tracing_to_file(
            "fleet_daemon=info,fleet_daemon_runtime=info",
            &log_path,
        )?;
    } else {
        fleet_core_common::init_tracing("fleet_daemon=info,fleet_daemon_runtime=info");
    }

    #[cfg(not(windows))]
    fleet_core_common::init_tracing("fleet_daemon=info,fleet_daemon_runtime=info");
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fleet_daemon::run(cli))
}

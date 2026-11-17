//! `fleet-daemon` process entrypoint.

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = fleet_daemon::Cli::parse();
    let filter = "fleet_daemon=info,fleet_daemon_runtime=info";
    let log_file = std::env::var_os("FLEET_LOG_FILE").map(std::path::PathBuf::from);

    #[cfg(windows)]
    {
        let log_path = if cli.windows_task {
            Some(log_file.unwrap_or_else(|| {
                cli.config
                    .as_deref()
                    .and_then(|path| path.parent())
                    .map(|dir| dir.join("logs").join("daemon.log"))
                    .unwrap_or_else(|| std::env::temp_dir().join("fleet-daemon.log"))
            }))
        } else {
            log_file
        };
        match log_path {
            Some(path) => fleet_core_common::init_tracing_to_file(filter, &path)?,
            None => fleet_core_common::init_tracing(filter),
        }
    }
    #[cfg(not(windows))]
    match log_file {
        Some(path) => fleet_core_common::init_tracing_to_file(filter, &path)?,
        None => fleet_core_common::init_tracing(filter),
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fleet_daemon::run(cli))
}

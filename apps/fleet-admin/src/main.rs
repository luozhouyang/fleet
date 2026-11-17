//! `fleet-admin` process entrypoint.

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = fleet_admin::Cli::parse();
    let filter = "fleet_admin=info";
    let log_file = std::env::var_os("FLEET_LOG_FILE").map(std::path::PathBuf::from);

    #[cfg(windows)]
    if cli.windows_service {
        let log_path = log_file.unwrap_or_else(|| {
            cli.config
                .as_deref()
                .and_then(|path| path.parent())
                .map(|dir| dir.join("logs").join("admin.log"))
                .unwrap_or_else(|| std::env::temp_dir().join("fleet-admin.log"))
        });
        fleet_core_common::init_tracing_to_file(filter, &log_path)?;
        fleet_admin::windows_service::run()?;
        return Ok(());
    }

    #[cfg(windows)]
    let log_file = if cli.windows_task {
        Some(log_file.unwrap_or_else(|| {
            cli.config
                .as_deref()
                .and_then(|path| path.parent())
                .map(|dir| dir.join("logs").join("admin.log"))
                .unwrap_or_else(|| std::env::temp_dir().join("fleet-admin.log"))
        }))
    } else {
        log_file
    };

    match log_file {
        Some(path) => fleet_core_common::init_tracing_to_file(filter, &path)?,
        None => fleet_core_common::init_tracing(filter),
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fleet_admin::run(cli))
}

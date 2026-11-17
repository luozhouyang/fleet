//! `fleet` — Fleet command line.

mod cli;
mod client;
mod cloudflare;
mod config;
mod config_commands;
mod config_paths;
mod connect;
mod doctor;
mod fleet_status;
mod install;
mod local;
mod macos_app;
mod network;
mod output;
mod pairing;
mod process;
mod prompt;
mod secrets;
mod setup;
mod uninstall;
mod upgrade;

#[tokio::main]
async fn main() {
    if let Err(error) = cli::run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

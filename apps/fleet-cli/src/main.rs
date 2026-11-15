//! `af` — Agent Fleet command line.

mod cli;
mod client;
mod cloudflare;
mod config;
mod connect;
mod network;
mod output;
mod pairing;
mod secrets;

#[tokio::main]
async fn main() {
    if let Err(error) = cli::run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

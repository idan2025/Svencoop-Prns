use anyhow::Result;
use clap::Parser;

use sc_rns_bridge::config::{Cli, Role};
use sc_rns_bridge::run_bridge;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sc_rns_bridge=info,personal_rns=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = match cli.role {
        Role::Server(args) => sc_rns_bridge::BridgeConfig::Server(args),
        Role::Client(args) => sc_rns_bridge::BridgeConfig::Client(args),
    };

    run_bridge(cfg).await
}
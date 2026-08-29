use std::path::PathBuf;

use clap::{Parser, Subcommand};
// `serde` derives are gated behind the `serde` feature so the CLI binary stays
// dependency-light; the controller enables it to persist these args.

#[derive(Parser, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[command(name = "sc-rns-bridge", about = "Sven Co-op over Reticulum")]
pub struct Cli {
    #[command(subcommand)]
    pub role: Role,
}

#[derive(Subcommand, Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Role {
    /// Announce a Reticulum destination and bridge accepted links to a local
    /// Sven Co-op server's UDP port.
    Server(ServerArgs),
    /// Bind a local UDP port that GoldSrc clients connect to and relay traffic
    /// over a Reticulum link to the announced server.
    Client(ClientArgs),
}

#[derive(Parser, Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ServerArgs {
    /// UDP port the real Sven Co-op server listens on.
    #[arg(long, default_value_t = 27015)]
    pub sc_port: u16,

    /// Host the Sven Co-op server runs on. Use 127.0.0.1 if co-located.
    #[arg(long, default_value = "127.0.0.1")]
    pub sc_host: String,

    /// Where to persist the server identity. Generated on first run.
    #[arg(long, default_value = "./sc-rns-server.identity")]
    pub identity: PathBuf,

    /// Optional TCP interface to attach, e.g. 0.0.0.0:4234 for a public relay.
    /// If omitted and --auto is off, the node has no interfaces and can't talk.
    #[arg(long)]
    pub tcp: Option<String>,

    /// Enable Wi-Fi/LAN auto-discovery for nearby peers.
    #[arg(long, default_value_t = false)]
    pub auto: bool,

    /// Announce interval in seconds.
    #[arg(long, default_value_t = 15)]
    pub announce_interval: u64,

    /// Optional display name broadcast in this server's announces, so
    /// clients can show something friendlier than a bare hash in the server
    /// browser. Falls back to a fixed default when unset.
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClientArgs {
    /// Local UDP port the GoldSrc client will connect to.
    #[arg(long, default_value_t = 27015)]
    pub listen_port: u16,

    /// Destination hash of the server to connect to, in hex (32 hex chars).
    /// If omitted, the client waits to hear an announce for the sven-coop/server
    /// destination and auto-connects to the first one it sees.
    #[arg(long)]
    pub server_hash: Option<String>,

    /// Where to persist the client identity. Generated on first run.
    #[arg(long, default_value = "./sc-rns-client.identity")]
    pub identity: PathBuf,

    /// Optional TCP interface to attach (client side of a public relay).
    #[arg(long)]
    pub tcp: Option<String>,

    /// Enable Wi-Fi/LAN auto-discovery for nearby peers.
    #[arg(long, default_value_t = false)]
    pub auto: bool,
}

#[derive(Debug, Clone)]
pub enum BridgeConfig {
    Server(ServerArgs),
    Client(ClientArgs),
}

impl BridgeConfig {
    pub fn role(&self) -> BridgeRole {
        match self {
            Self::Server(_) => BridgeRole::Server,
            Self::Client(_) => BridgeRole::Client,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeRole {
    Server,
    Client,
}
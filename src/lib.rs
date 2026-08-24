//! Sven Co-op over Reticulum bridge.
//!
//! Two daemons share one engine here:
//!  - `server` announces a Reticulum destination and bridges each accepted Link
//!    to a UDP socket that talks to a local Sven Co-op server.
//!  - `client` binds 127.0.0.1:27015 and, when a GoldSrc client sends the first
//!    packet, opens a Link to the announced server destination and relays both
//!    ways until the link closes.
//!
//! The wire contract is intentionally dumb: each Reticulum link packet carries
//! one raw GoldSrc UDP datagram, bytes-in-bytes-out. The Link itself supplies
//! encryption, ordering, and integrity; Reticulum supplies the routing.

pub mod config;
pub mod framing;
pub mod relay;

pub use config::{BridgeConfig, BridgeRole};
pub use relay::run_bridge;

pub const SC_APP_NAME: &str = "sven-coop";
pub const SC_ASPECT_SERVER: &str = "server";
pub const SC_ASPECT_CLIENT: &str = "client";
pub const DEFAULT_LISTEN_PORT: u16 = 27015;
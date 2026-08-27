//! A discovered `sven-coop/server` destination, for the server browser.

use serde::Serialize;

use sc_rns_bridge::DiscoveredServer;

/// One row in the server browser: a Reticulum `sven-coop.server` destination
/// we've heard an announce from, with how long ago we last heard it.
///
/// Built from the bridge's internal `DiscoveredServer` into the serializable
/// shape the GUI/frontend consumes. `last_seen_ago_secs` is computed at
/// snapshot time (`Instant::elapsed`), so it's a moment-in-time view, not live.
#[derive(Debug, Clone, Serialize)]
pub struct ServerEntry {
    /// 32-hex-char destination hash.
    pub destination_hash: String,
    /// Seconds since we last heard an announce from this server.
    pub last_seen_ago_secs: u64,
}

impl ServerEntry {
    pub fn from_discovered(d: &DiscoveredServer) -> Self {
        Self {
            destination_hash: hex::encode(d.destination_hash.as_bytes()),
            last_seen_ago_secs: d.last_seen.elapsed().as_secs(),
        }
    }
}
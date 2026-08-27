//! The platform orchestrator: owns a running [`BridgeSession`] (server or
//! client role), a [`DsManager`], and ties them together for the GUI.
//!
//! The GUI (a thin Tauri shell) holds this behind an `Arc<Mutex<>>` and calls
//! one method per user action. Every method is `async + Result`-returning so
//! the Tauri commands are trivial wrappers. The bridge session itself drives
//! its node on a dedicated thread (see `sc_rns_bridge::relay`), so this struct
//! is `Send` and safe to share.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use tokio::net::UdpSocket;

use personal_rns::prelude::*;
use prns_core::interfaces::{ConnectionState, DEFAULT_IFAC_SIZE, IfacContext, InterfaceId};

use sc_rns_bridge::{BridgeSession, ClientArgs, ServerArgs};

use crate::ds::{DsManager, DsStartArgs, DsStatus};
use crate::game::GameLauncher;
use crate::server_entry::ServerEntry;

/// One attached Reticulum interface, for the GUI's Interfaces tab.
#[derive(Debug, Clone, Serialize)]
pub struct InterfaceInfo {
    /// Interface id as hex (stable identifier for remove/rename).
    pub id: String,
    /// Human-readable name, if set.
    pub name: Option<String>,
    /// Interface mode (e.g. Access, Focus, Transport) — `Debug` render.
    pub mode: String,
    /// Connection state (e.g. Connected, Disconnected) — `Debug` render.
    pub connection: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub links: u32,
}

/// A poll-able snapshot of everything the GUI shows at once.
#[derive(Debug, Clone, Serialize)]
pub struct ControllerState {
    pub bridge_running: bool,
    pub bridge_role: Option<String>,
    pub ds: DsStatus,
    pub servers: Vec<ServerEntry>,
    pub interfaces: Vec<InterfaceInfo>,
}

/// One running bridge + the DS manager.
pub struct BridgeController {
    bundle_dir: PathBuf,
    ds: DsManager,
    session: Option<BridgeSession>,
    last_server_args: Option<ServerArgs>,
    last_client_args: Option<ClientArgs>,
}

impl BridgeController {
    pub fn new(bundle_dir: PathBuf) -> Self {
        Self {
            ds: DsManager::new(bundle_dir.clone()),
            bundle_dir,
            session: None,
            last_server_args: None,
            last_client_args: None,
        }
    }

    /// The bundle dir everything resolves from.
    pub fn bundle_dir(&self) -> &std::path::Path {
        &self.bundle_dir
    }

    /// True if a bridge session (either role) is running.
    pub fn is_running(&self) -> bool {
        self.session.is_some()
    }

    fn require_session(&self) -> Result<&BridgeSession> {
        self.session
            .as_ref()
            .ok_or_else(|| anyhow!("no bridge session is running"))
    }

    // ---- bridge server ----

    pub async fn start_bridge_server(&mut self, args: ServerArgs) -> Result<()> {
        if self.session.is_some() {
            anyhow::bail!("a bridge session is already running; stop it first");
        }
        self.session = Some(BridgeSession::start_server(args.clone()).await?);
        self.last_server_args = Some(args);
        Ok(())
    }

    pub async fn stop_bridge_server(&mut self) -> Result<()> {
        if let Some(mut s) = self.session.take() {
            s.stop();
        }
        Ok(())
    }

    pub async fn restart_bridge_server(&mut self) -> Result<()> {
        let args = self
            .last_server_args
            .clone()
            .ok_or_else(|| anyhow!("no previous bridge server start to restart from"))?;
        self.stop_bridge_server().await?;
        self.start_bridge_server(args).await
    }

    // ---- bridge client ----

    pub async fn start_client(&mut self, args: ClientArgs) -> Result<()> {
        if self.session.is_some() {
            anyhow::bail!("a bridge session is already running; stop it first");
        }
        self.session = Some(BridgeSession::start_client(args.clone()).await?);
        self.last_client_args = Some(args);
        Ok(())
    }

    pub async fn stop_client(&mut self) -> Result<()> {
        if let Some(mut s) = self.session.take() {
            s.stop();
        }
        Ok(())
    }

    pub async fn restart_client(&mut self) -> Result<()> {
        let args = self
            .last_client_args
            .clone()
            .ok_or_else(|| anyhow!("no previous client start to restart from"))?;
        self.stop_client().await?;
        self.start_client(args).await
    }

    // ---- server browser ----

    pub async fn list_servers(&self) -> Result<Vec<ServerEntry>> {
        match &self.session {
            Some(s) => Ok(s.discovered().await.iter().map(ServerEntry::from_discovered).collect()),
            None => Ok(Vec::new()),
        }
    }

    // ---- live interface control ----

    pub fn list_interfaces(&self) -> Result<Vec<InterfaceInfo>> {
        let handle = self.require_session()?.handle();
        Ok(handle
            .interface_inventory()
            .into_iter()
            .map(|e| InterfaceInfo {
                id: hex::encode(e.snapshot.id.as_bytes()),
                name: e.name,
                mode: format!("{:?}", e.snapshot.mode),
                connection: format!("{:?}", e.snapshot.connection),
                rx_bytes: e.snapshot.rx_bytes,
                tx_bytes: e.snapshot.tx_bytes,
                links: e.snapshot.links,
            })
            .collect())
    }

    /// Add a TCP interface. `addr` is `host:port`; `0.0.0.0:PORT` (or `:PORT`)
    /// binds a TCP server, any other host connects as a TCP client. If
    /// `ifac_name` is given, the interface is IFAC-protected with that network
    /// name.
    pub async fn add_interface_tcp(&self, addr: String, ifac_name: Option<String>) -> Result<()> {
        let handle = self.require_session()?.handle();
        let (host, port) = parse_host_port(&addr)?;
        // IfacContext::derive returns None when the network name is empty/None,
        // which is exactly the "no IFAC" case.
        let ifac = IfacContext::derive(ifac_name.as_deref(), None, DEFAULT_IFAC_SIZE);
        let ifac_set = ifac.is_some();
        if host == "0.0.0.0" || host.is_empty() {
            // Bind a TCP server interface.
            let srv = TcpServer::bind(&addr)
                .await
                .with_context(|| format!("binding TCP server on {addr}"))?;
            match ifac {
                Some(ifac) => {
                    handle.supervise_with_ifac_name(srv, ifac, None);
                }
                None => {
                    handle.supervise(srv);
                }
            }
            tracing::info!(tcp = %addr, ifac = ifac_set, "attached TCP server interface");
        } else if port > 0 {
            let client = TcpClientInterface::new(addr.clone());
            match ifac {
                Some(ifac) => {
                    handle.add_interface_with_ifac_name(client, ifac, None);
                }
                None => {
                    handle.add_interface(client);
                }
            }
            tracing::info!(tcp = %addr, ifac = ifac_set, "attached TCP client interface");
        } else {
            return Err(anyhow!("invalid TCP address {addr}: no port"));
        }
        Ok(())
    }

    /// Add a Wi-Fi/LAN auto-discovery interface (no internet needed).
    pub fn add_interface_auto(&self) -> Result<()> {
        let handle = self.require_session()?.handle();
        handle.attach(AutoWifi::default());
        tracing::info!("attached Wi-Fi/LAN auto-discovery interface");
        Ok(())
    }

    /// Remove an interface by its hex id.
    pub fn remove_interface(&self, id_hex: &str) -> Result<()> {
        let handle = self.require_session()?.handle();
        let id = find_interface_by_hex(handle, id_hex)?
            .ok_or_else(|| anyhow!("no interface with id {id_hex}"))?;
        handle.remove_interface(id);
        tracing::info!(id = id_hex, "removed interface");
        Ok(())
    }

    /// Rename an interface by its hex id.
    pub fn rename_interface(&self, id_hex: &str, name: String) -> Result<()> {
        let handle = self.require_session()?.handle();
        let id = find_interface_by_hex(handle, id_hex)?
            .ok_or_else(|| anyhow!("no interface with id {id_hex}"))?;
        if !handle.set_interface_name(id, name.clone()) {
            return Err(anyhow!("failed to rename interface {id_hex}"));
        }
        tracing::info!(id = id_hex, name = %name, "renamed interface");
        Ok(())
    }

    // ---- dedicated server ----

    pub async fn ds_start(&mut self, args: DsStartArgs) -> Result<()> {
        self.ds.start(args).await
    }

    pub async fn ds_stop(&mut self) -> Result<()> {
        self.ds.stop().await
    }

    pub async fn ds_status(&mut self) -> DsStatus {
        self.ds.status().await
    }

    // ---- connect + launch ----

    /// Ensure a client session is running pointed at `server_hash_hex`, wait
    /// for its listen port to bind, then open the Sven Co-op game
    /// auto-connected to that port. This is the "click a server → play" path.
    ///
    /// Requires a client to have been started at least once (so we know the
    /// listen port / interface / identity to reuse); it restarts the client
    /// with the clicked server's hash.
    pub async fn connect_and_launch(&mut self, server_hash_hex: String) -> Result<()> {
        let mut args = self
            .last_client_args
            .clone()
            .ok_or_else(|| anyhow!("start a client first (set its listen port + interface), then click Connect"))?;
        // Point the client at the clicked server.
        validate_hex_hash(&server_hash_hex)?;
        args.server_hash = Some(server_hash_hex);

        // Restart the client with the new hash (if it was running, stop first).
        self.stop_client().await?;
        self.start_client(args.clone()).await?;

        // Wait for the bridge client's UDP listener to bind on 127.0.0.1:listen_port.
        wait_for_udp_bind(args.listen_port, Duration::from_secs(8)).await?;

        GameLauncher::launch(args.listen_port)
    }

    // ---- snapshot ----

    /// Poll-able snapshot of the whole UI state.
    pub async fn state(&mut self) -> Result<ControllerState> {
        let (bridge_running, bridge_role) = match &self.session {
            Some(s) => (true, Some(format!("{:?}", s.role()))),
            None => (false, None),
        };
        let servers = self.list_servers().await?;
        let interfaces = self.list_interfaces().unwrap_or_default();
        let ds = self.ds.status().await;
        Ok(ControllerState {
            bridge_running,
            bridge_role,
            ds,
            servers,
            interfaces,
        })
    }
}

/// Parse `host:port`. Returns (host, port). Errors on a missing port.
fn parse_host_port(addr: &str) -> Result<(String, u16)> {
    let colon = addr
        .rfind(':')
        .ok_or_else(|| anyhow!("TCP address {addr} has no port"))?;
    let host = addr[..colon].to_string();
    let port: u16 = addr[colon + 1..]
        .parse()
        .with_context(|| format!("TCP address {addr} has a non-numeric port"))?;
    Ok((host, port))
}

/// Find an interface id on the running node whose hex matches `id_hex`.
fn find_interface_by_hex(handle: &PrnsNodeHandle, id_hex: &str) -> Result<Option<InterfaceId>> {
    let target = id_hex.to_ascii_lowercase();
    for snap in handle.interfaces() {
        if hex::encode(snap.id.as_bytes()) == target {
            return Ok(Some(snap.id));
        }
    }
    Ok(None)
}

/// Validate a 32-hex-char destination hash (16 bytes).
fn validate_hex_hash(hex_str: &str) -> Result<()> {
    let bytes = hex::decode(hex_str.trim()).map_err(|e| anyhow!("invalid server hash: {e}"))?;
    if bytes.len() != 16 {
        return Err(anyhow!(
            "server hash must be 16 bytes (32 hex chars), got {} bytes",
            bytes.len()
        ));
    }
    Ok(())
}

/// Wait until something is bound to 127.0.0.1:`port` (i.e. our client has it).
/// We detect this by trying to bind it ourselves: AddrInUse means the client
/// owns it → ready. Success means the client hasn't bound yet → retry.
async fn wait_for_udp_bind(port: u16, timeout: Duration) -> Result<()> {
    let start = std::time::Instant::now();
    loop {
        // Try to bind the port ourselves.
        match UdpSocket::bind(format!("127.0.0.1:{port}")).await {
            Ok(_sock) => {
                // We bound it → the client hasn't yet. Drop and retry.
                drop(_sock);
            }
            Err(e) if e.to_string().to_lowercase().contains("in use") => {
                return Ok(());
            }
            Err(e) => {
                // Some errors (e.g. permission) are still a "port is taken" signal
                // on some platforms; treat any bind failure other than retry as
                // possibly-in-use and check again.
                tracing::debug!(error = %e, port, "UDP bind probe failed; retrying");
            }
        }
        if start.elapsed() >= timeout {
            return Err(anyhow!(
                "timed out waiting for bridge client to bind UDP port {port}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

// Silence unused import warnings for re-exports the GUI will consume.
#[allow(unused_imports)]
use personal_rns::prelude as _prelude_re_exports;
#[allow(unused_imports)]
use ConnectionState as _ConnectionState;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_port_splits() {
        let (h, p) = parse_host_port("0.0.0.0:4234").unwrap();
        assert_eq!(h, "0.0.0.0");
        assert_eq!(p, 4234);
        let (h, p) = parse_host_port("example.com:4234").unwrap();
        assert_eq!(h, "example.com");
        assert_eq!(p, 4234);
        assert!(parse_host_port("noport").is_err());
        assert!(parse_host_port("host:abc").is_err());
    }

    #[test]
    fn validate_hex_hash_accepts_32_hex() {
        // 32 hex chars = 16 bytes = a valid destination hash.
        assert!(validate_hex_hash("ffffffffffffffffffffffffffffffff").is_ok());
        assert!(validate_hex_hash("deadbeef").is_err()); // too short
        assert!(validate_hex_hash("zzffffffffffffffffffffffffffffff").is_err()); // not hex
    }

    #[test]
    fn connect_uri_connects_to_listen_port() {
        assert!(GameLauncher::connect_uri(27016).contains(":27016"));
    }
}
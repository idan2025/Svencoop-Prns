//! The platform orchestrator: owns up to two running [`BridgeSession`]s —
//! one server-role and one client-role, independent of each other — plus a
//! [`DsManager`], and ties them together for the GUI. Server and client are
//! separate Reticulum nodes (own identity, own interfaces) under the hood,
//! so running both at once just needs non-conflicting ports.
//!
//! The GUI (a thin Tauri shell) holds this behind an `Arc<Mutex<>>` and calls
//! one method per user action. Every method is `async + Result`-returning so
//! the Tauri commands are trivial wrappers. The bridge session itself drives
//! its node on a dedicated thread (see `sc_rns_bridge::relay`), so this struct
//! is `Send` and safe to share.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

use personal_rns::prelude::*;
use prns_core::interfaces::udp::UDP_BITRATE_ESTIMATE;
use prns_core::interfaces::websocket::{WebSocketFramingSelection, WEBSOCKET_BITRATE_ESTIMATE};
use prns_core::interfaces::{ConnectionState, DEFAULT_IFAC_SIZE, IfacContext, InterfaceId};

use sc_rns_bridge::{BridgeSession, ClientArgs, ServerArgs};

use crate::ds::{DsManager, DsStartArgs, DsStatus};
use crate::game::GameLauncher;
use crate::server_entry::ServerEntry;

/// One saved Reticulum interface, for re-attaching on resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceDescriptor {
    /// Runtime interface id (hex) captured at attach time; used to match a
    /// `remove_interface` call to this descriptor. Re-generated on resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `"tcp"`, `"udp"`, `"websocket"`, or `"auto"`.
    pub kind: String,
    /// Which running session this interface is attached to: `"server"` or
    /// `"client"`. Defaults to `"server"` for descriptors saved before dual
    /// sessions existed, when there was only ever one to attach to.
    #[serde(default = "default_role")]
    pub role: String,
    /// `host:port` for tcp; the local bind `host:port` for udp; a
    /// `ws://`/`wss://` target URL (client) or bind `host:port` (server) for
    /// websocket; absent for auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    /// Remote peer `host:port` for udp only (UDP is a fixed-peer point
    /// interface, so it needs both ends). Unused for other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_addr: Option<String>,
    /// IFAC network name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ifac_name: Option<String>,
    /// IFAC passphrase, if any. Stored in plaintext in `settings.json`
    /// alongside the rest of the local, unencrypted operator config — same
    /// trust boundary as an OS-level Wi-Fi password store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ifac_passphrase: Option<String>,
}

fn default_role() -> String {
    "server".to_string()
}

/// Persisted operator choices, written to `<bundle>/settings.json` so the host
/// resumes its last state after a container restart/recreate (as long as the
/// volume isn't wiped). Best-effort: a missing/corrupt file is treated as
/// defaults, never fatal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Schema version for future migrations.
    #[serde(default)]
    pub version: u32,
    /// Last bridge-server args (the host runs the server role).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge: Option<ServerArgs>,
    /// Whether the bridge server should be running on resume.
    #[serde(default)]
    pub bridge_running: bool,
    /// Last bridge-client args, independent of the server (the same process
    /// can run both roles at once, on different ports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientArgs>,
    /// Whether the bridge client should be running on resume.
    #[serde(default)]
    pub client_running: bool,
    /// Last dedicated-server args.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ds: Option<DsStartArgs>,
    /// Whether the DS should be running on resume.
    #[serde(default)]
    pub ds_running: bool,
    /// Extra live-attached Reticulum interfaces to re-attach on resume.
    #[serde(default)]
    pub interfaces: Vec<InterfaceDescriptor>,
}

impl Settings {
    const VERSION: u32 = 1;

    fn new() -> Self {
        Self {
            version: Self::VERSION,
            ..Default::default()
        }
    }
}

impl InterfaceDescriptor {
    fn addr_or_kind(&self) -> String {
        self.addr.clone().unwrap_or_else(|| self.kind.clone())
    }
}

/// Load settings from a path; returns None if missing or unreadable (best-effort).
fn load_settings(path: &PathBuf) -> Option<Settings> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "settings.json corrupt; ignoring");
            None
        }
    }
}

/// One attached Reticulum interface, for the GUI's Interfaces tab.
#[derive(Debug, Clone, Serialize)]
pub struct InterfaceInfo {
    /// Interface id as hex (stable identifier for remove/rename).
    pub id: String,
    /// Which session this interface belongs to: `"server"` or `"client"`.
    pub role: String,
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
    /// True if either the server or the client session is running.
    pub bridge_running: bool,
    /// `"Server"`, `"Client"`, `"Server + Client"`, or `None` if neither is
    /// running — for the status pill.
    pub bridge_role: Option<String>,
    pub server_running: bool,
    pub client_running: bool,
    pub ds: DsStatus,
    pub servers: Vec<ServerEntry>,
    pub interfaces: Vec<InterfaceInfo>,
    /// Warnings from the last `resume()` (e.g. a saved component failed to
    /// restart). Empty when everything resumed cleanly.
    #[serde(default)]
    pub resume_errors: Vec<String>,
}

/// Up to two running bridge sessions (server + client, independent) + the DS
/// manager.
pub struct BridgeController {
    bundle_dir: PathBuf,
    ds: DsManager,
    server_session: Option<BridgeSession>,
    client_session: Option<BridgeSession>,
    last_server_args: Option<ServerArgs>,
    last_client_args: Option<ClientArgs>,
    settings: Settings,
    resume_errors: Vec<String>,
}

impl BridgeController {
    pub fn new(bundle_dir: PathBuf) -> Self {
        let settings_path = bundle_dir.join("settings.json");
        let settings = load_settings(&settings_path).unwrap_or_else(Settings::new);
        // Seed the in-memory "last args" from persisted settings so the panel
        // opens showing the real last config, not defaults.
        let last_server_args = settings.bridge.clone();
        let last_client_args = settings.client.clone();
        Self {
            ds: DsManager::new(bundle_dir.clone()),
            bundle_dir,
            server_session: None,
            client_session: None,
            last_server_args,
            last_client_args,
            settings,
            resume_errors: Vec::new(),
        }
    }

    /// The bundle dir everything resolves from.
    pub fn bundle_dir(&self) -> &std::path::Path {
        &self.bundle_dir
    }

    fn settings_path(&self) -> PathBuf {
        self.bundle_dir.join("settings.json")
    }

    /// Best-effort persist of the current settings to `<bundle>/settings.json`.
    /// Logs on failure; never returns an error (settings are non-critical).
    fn save_settings(&self) {
        let path = self.settings_path();
        match serde_json::to_string_pretty(&self.settings) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!(path = %path.display(), error = %e, "failed to write settings.json");
                }
            }
            Err(e) => tracing::warn!(error = %e, "failed to serialize settings"),
        }
    }

    /// Restore the last running state from `settings.json`. Called once on
    /// startup. Failures are logged + recorded in `resume_errors`, never fatal
    /// — the panel still loads and the operator can fix things from the UI.
    pub async fn resume(&mut self) {
        self.resume_errors.clear();
        let s = self.settings.clone();

        // Bridge server and client first (interfaces attach to one or the
        // other's node) — independent of each other, so both get a chance
        // to come up even if one fails.
        if s.bridge_running {
            if let Some(args) = s.bridge.clone() {
                if let Err(e) = self.start_bridge_server(args).await {
                    self.resume_errors.push(format!("bridge server: {e}"));
                }
            }
        }
        if s.client_running {
            if let Some(args) = s.client.clone() {
                if let Err(e) = self.start_client(args).await {
                    self.resume_errors.push(format!("bridge client: {e}"));
                }
            }
        }

        // Re-attach saved extra interfaces to whichever session each one was
        // recorded against.
        for desc in s.interfaces.clone() {
            let res = match desc.kind.as_str() {
                "tcp" => {
                    let addr = desc.addr.clone().unwrap_or_default();
                    self.add_interface_tcp(addr, desc.role.clone(), desc.ifac_name.clone(), desc.ifac_passphrase.clone())
                        .await
                }
                "auto" => self.add_interface_auto(desc.role.clone(), desc.ifac_name.clone(), desc.ifac_passphrase.clone()),
                "udp" => {
                    let local = desc.addr.clone().unwrap_or_default();
                    let peer = desc.peer_addr.clone().unwrap_or_default();
                    self.add_interface_udp(local, peer, desc.role.clone(), desc.ifac_name.clone(), desc.ifac_passphrase.clone())
                        .await
                }
                "websocket" => {
                    let addr = desc.addr.clone().unwrap_or_default();
                    self.add_interface_websocket(addr, desc.role.clone(), desc.ifac_name.clone(), desc.ifac_passphrase.clone())
                        .await
                }
                other => Err(anyhow!("unknown interface kind {other} in settings")),
            };
            if let Err(e) = res {
                self.resume_errors.push(format!("interface {:?}: {e}", desc.addr_or_kind()));
            }
        }

        // Dedicated server last.
        if s.ds_running {
            if let Some(args) = s.ds.clone() {
                if let Err(e) = self.ds_start(args).await {
                    self.resume_errors.push(format!("dedicated server: {e}"));
                }
            }
        }

        if self.resume_errors.is_empty() {
            tracing::info!("resume: restored last running state from settings.json");
        } else {
            tracing::warn!(errors = ?self.resume_errors, "resume: some components failed to restart");
        }
    }

    /// True if either bridge session (server or client) is running.
    pub fn is_running(&self) -> bool {
        self.server_session.is_some() || self.client_session.is_some()
    }

    /// Look up a running session by role string (`"server"` or `"client"`).
    fn session(&self, role: &str) -> Result<&BridgeSession> {
        match role {
            "server" => self
                .server_session
                .as_ref()
                .ok_or_else(|| anyhow!("no bridge server session is running")),
            "client" => self
                .client_session
                .as_ref()
                .ok_or_else(|| anyhow!("no bridge client session is running")),
            other => Err(anyhow!("unknown role {other:?}, expected \"server\" or \"client\"")),
        }
    }

    // ---- bridge server ----

    pub async fn start_bridge_server(&mut self, args: ServerArgs) -> Result<()> {
        if self.server_session.is_some() {
            anyhow::bail!("a bridge server session is already running; stop it first");
        }
        self.server_session = Some(BridgeSession::start_server(args.clone()).await?);
        self.last_server_args = Some(args.clone());
        self.settings.bridge = Some(args);
        self.settings.bridge_running = true;
        self.save_settings();
        Ok(())
    }

    pub async fn stop_bridge_server(&mut self) -> Result<()> {
        if let Some(mut s) = self.server_session.take() {
            s.stop();
        }
        self.settings.bridge_running = false;
        self.save_settings();
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
        if self.client_session.is_some() {
            anyhow::bail!("a bridge client session is already running; stop it first");
        }
        self.client_session = Some(BridgeSession::start_client(args.clone()).await?);
        self.last_client_args = Some(args.clone());
        self.settings.client = Some(args);
        self.settings.client_running = true;
        self.save_settings();
        Ok(())
    }

    pub async fn stop_client(&mut self) -> Result<()> {
        if let Some(mut s) = self.client_session.take() {
            s.stop();
        }
        self.settings.client_running = false;
        self.save_settings();
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

    /// Discovered server announces. Sourced from the client session (that's
    /// the role that's browsing for hosts to connect to); falls back to the
    /// server session's own view if only a server is running, since its node
    /// still passively hears announces on whatever interfaces it has.
    pub async fn list_servers(&self) -> Result<Vec<ServerEntry>> {
        let session = self.client_session.as_ref().or(self.server_session.as_ref());
        match session {
            Some(s) => Ok(s.discovered().await.iter().map(ServerEntry::from_discovered).collect()),
            None => Ok(Vec::new()),
        }
    }

    // ---- live interface control ----

    /// Interfaces from both sessions, each tagged with which one it belongs
    /// to — a server and a client running at once are two separate nodes,
    /// each with their own interfaces.
    pub fn list_interfaces(&self) -> Result<Vec<InterfaceInfo>> {
        let mut out = Vec::new();
        for (role, session) in [("server", &self.server_session), ("client", &self.client_session)] {
            let Some(session) = session else { continue };
            let handle = session.handle();
            out.extend(handle.interface_inventory().into_iter().map(|e| InterfaceInfo {
                id: hex::encode(e.snapshot.id.as_bytes()),
                role: role.to_string(),
                name: e.name,
                mode: format!("{:?}", e.snapshot.mode),
                connection: format!("{:?}", e.snapshot.connection),
                rx_bytes: e.snapshot.rx_bytes,
                tx_bytes: e.snapshot.tx_bytes,
                links: e.snapshot.links,
            }));
        }
        Ok(out)
    }

    /// Add a TCP interface. `addr` is `host:port`; `0.0.0.0:PORT` (or `:PORT`)
    /// binds a TCP server, any other host connects as a TCP client. If
    /// `ifac_name` and/or `ifac_passphrase` are given, the interface is
    /// IFAC-protected — matching upstream Reticulum, either one alone is
    /// enough to derive a key, but the passphrase is the actual secret (the
    /// network name alone is guessable/enumerable, e.g. from packet metadata
    /// or by anyone who's seen it once). Persists the interface so it is
    /// re-attached on resume.
    pub async fn add_interface_tcp(
        &mut self,
        addr: String,
        role: String,
        ifac_name: Option<String>,
        ifac_passphrase: Option<String>,
    ) -> Result<()> {
        let handle = self.session(&role)?.handle();
        let (host, port) = parse_host_port(&addr)?;
        let before: HashSet<InterfaceId> = handle.interfaces().iter().map(|s| s.id).collect();
        // IfacContext::derive returns None when both name and passphrase are
        // empty/None, which is exactly the "no IFAC" case.
        let ifac = IfacContext::derive(
            ifac_name.as_deref(),
            ifac_passphrase.as_deref(),
            DEFAULT_IFAC_SIZE,
        );
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
        // Capture the newly-attached interface's runtime id (before/after diff)
        // so a later `remove_interface` can drop the matching saved descriptor.
        let new_id = handle
            .interfaces()
            .iter()
            .find(|s| !before.contains(&s.id))
            .map(|s| hex::encode(s.id.as_bytes()));
        self.settings.interfaces.push(InterfaceDescriptor {
            id: new_id,
            kind: "tcp".to_string(),
            addr: Some(addr),
            peer_addr: None,
            role,
            ifac_name,
            ifac_passphrase,
        });
        self.save_settings();
        Ok(())
    }

    /// Add a Wi-Fi/LAN auto-discovery interface (no internet needed). If
    /// `ifac_name` and/or `ifac_passphrase` are given, only peers sharing the
    /// same IFAC can talk to this interface — otherwise any node on the LAN
    /// segment discovering the multicast announce can attach. Persists it so
    /// it is re-attached on resume.
    pub fn add_interface_auto(
        &mut self,
        role: String,
        ifac_name: Option<String>,
        ifac_passphrase: Option<String>,
    ) -> Result<()> {
        let handle = self.session(&role)?.handle();
        let before: HashSet<InterfaceId> = handle.interfaces().iter().map(|s| s.id).collect();
        let ifac = IfacContext::derive(
            ifac_name.as_deref(),
            ifac_passphrase.as_deref(),
            DEFAULT_IFAC_SIZE,
        );
        match ifac {
            Some(ifac) => {
                handle.attach_with_ifac_name(AutoWifi::default(), ifac, ifac_name.clone());
            }
            None => {
                handle.attach(AutoWifi::default());
            }
        }
        let new_id = handle
            .interfaces()
            .iter()
            .find(|s| !before.contains(&s.id))
            .map(|s| hex::encode(s.id.as_bytes()));
        tracing::info!(ifac = ifac_name.is_some() || ifac_passphrase.is_some(), "attached Wi-Fi/LAN auto-discovery interface");
        self.settings.interfaces.push(InterfaceDescriptor {
            id: new_id,
            kind: "auto".to_string(),
            addr: None,
            peer_addr: None,
            role,
            ifac_name,
            ifac_passphrase,
        });
        self.save_settings();
        Ok(())
    }

    /// Add a UDP interface to the given session (`"server"` or `"client"`).
    /// Unlike TCP, a UDP link has no bind-vs-connect distinction of its own —
    /// it's a fixed point-to-point link, so both a local bind address and
    /// the remote peer's address are required. If `ifac_name` and/or
    /// `ifac_passphrase` are given, the interface is IFAC-protected.
    /// Persists the interface so it is re-attached on resume.
    pub async fn add_interface_udp(
        &mut self,
        local: String,
        peer: String,
        role: String,
        ifac_name: Option<String>,
        ifac_passphrase: Option<String>,
    ) -> Result<()> {
        let handle = self.session(&role)?.handle();
        let before: HashSet<InterfaceId> = handle.interfaces().iter().map(|s| s.id).collect();
        let ifac = IfacContext::derive(
            ifac_name.as_deref(),
            ifac_passphrase.as_deref(),
            DEFAULT_IFAC_SIZE,
        );
        let ifac_set = ifac.is_some();
        let iface = UdpInterface::bind(&local, &peer, UDP_BITRATE_ESTIMATE)
            .await
            .with_context(|| format!("binding UDP interface {local} <-> {peer}"))?;
        match ifac {
            Some(ifac) => {
                handle.add_interface_with_ifac_name(iface, ifac, ifac_name.clone());
            }
            None => {
                handle.add_interface(iface);
            }
        }
        tracing::info!(local = %local, peer = %peer, ifac = ifac_set, "attached UDP interface");
        let new_id = handle
            .interfaces()
            .iter()
            .find(|s| !before.contains(&s.id))
            .map(|s| hex::encode(s.id.as_bytes()));
        self.settings.interfaces.push(InterfaceDescriptor {
            id: new_id,
            kind: "udp".to_string(),
            addr: Some(local),
            peer_addr: Some(peer),
            role,
            ifac_name,
            ifac_passphrase,
        });
        self.save_settings();
        Ok(())
    }

    /// Add a WebSocket interface to the given session (`"server"` or
    /// `"client"`). `addr` starting with `ws://` or `wss://` is treated as a
    /// client target URL; otherwise it's parsed as `host:port` to bind a
    /// server (same convention as TCP). If `ifac_name` and/or
    /// `ifac_passphrase` are given, the interface is IFAC-protected.
    /// Persists the interface so it is re-attached on resume.
    pub async fn add_interface_websocket(
        &mut self,
        addr: String,
        role: String,
        ifac_name: Option<String>,
        ifac_passphrase: Option<String>,
    ) -> Result<()> {
        let handle = self.session(&role)?.handle();
        let before: HashSet<InterfaceId> = handle.interfaces().iter().map(|s| s.id).collect();
        let ifac = IfacContext::derive(
            ifac_name.as_deref(),
            ifac_passphrase.as_deref(),
            DEFAULT_IFAC_SIZE,
        );
        let ifac_set = ifac.is_some();
        if addr.starts_with("ws://") || addr.starts_with("wss://") {
            let client = WebSocketClientInterface::new(
                addr.clone(),
                WEBSOCKET_BITRATE_ESTIMATE,
                ReconnectPolicy::STANDARD,
                WebSocketFramingSelection::Auto,
            );
            match ifac {
                Some(ifac) => {
                    handle.add_interface_with_ifac_name(client, ifac, ifac_name.clone());
                }
                None => {
                    handle.add_interface(client);
                }
            }
            tracing::info!(ws = %addr, ifac = ifac_set, "attached WebSocket client interface");
        } else {
            let (_host, port) = parse_host_port(&addr)?;
            if port == 0 {
                return Err(anyhow!("invalid WebSocket bind address {addr}: no port"));
            }
            let srv = WebSocketServer::bind(
                &addr,
                WEBSOCKET_BITRATE_ESTIMATE,
                WebSocketFramingSelection::Auto,
            )
            .await
            .with_context(|| format!("binding WebSocket server on {addr}"))?;
            match ifac {
                Some(ifac) => {
                    handle.supervise_with_ifac_name(srv, ifac, None);
                }
                None => {
                    handle.supervise(srv);
                }
            }
            tracing::info!(ws = %addr, ifac = ifac_set, "attached WebSocket server interface");
        }
        let new_id = handle
            .interfaces()
            .iter()
            .find(|s| !before.contains(&s.id))
            .map(|s| hex::encode(s.id.as_bytes()));
        self.settings.interfaces.push(InterfaceDescriptor {
            id: new_id,
            kind: "websocket".to_string(),
            addr: Some(addr),
            peer_addr: None,
            role,
            ifac_name,
            ifac_passphrase,
        });
        self.save_settings();
        Ok(())
    }

    /// Remove an interface by its hex id. Also drops the matching saved
    /// descriptor so it won't be re-attached on resume.
    /// Which running session (if any) owns the interface with this hex id —
    /// checks both, so remove/rename don't need the caller to know or
    /// remember which role an interface was attached under.
    fn session_owning_interface(&self, id_hex: &str) -> Option<&BridgeSession> {
        [&self.server_session, &self.client_session]
            .into_iter()
            .flatten()
            .find(|s| matches!(find_interface_by_hex(s.handle(), id_hex), Ok(Some(_))))
    }

    pub fn remove_interface(&mut self, id_hex: &str) -> Result<()> {
        let handle = self
            .session_owning_interface(id_hex)
            .ok_or_else(|| anyhow!("no interface with id {id_hex}"))?
            .handle();
        let id = find_interface_by_hex(handle, id_hex)?
            .ok_or_else(|| anyhow!("no interface with id {id_hex}"))?;
        handle.remove_interface(id);
        let target = id_hex.to_ascii_lowercase();
        // Drop the saved descriptor whose captured id matches the removed one.
        self.settings.interfaces.retain(|d| {
            !d.id
                .as_deref()
                .map(|i| i.eq_ignore_ascii_case(&target))
                .unwrap_or(false)
        });
        tracing::info!(id = id_hex, "removed interface");
        self.save_settings();
        Ok(())
    }

    /// Rename an interface by its hex id.
    pub fn rename_interface(&self, id_hex: &str, name: String) -> Result<()> {
        let handle = self
            .session_owning_interface(id_hex)
            .ok_or_else(|| anyhow!("no interface with id {id_hex}"))?
            .handle();
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
        self.ds.start(args.clone()).await?;
        self.settings.ds = Some(args);
        self.settings.ds_running = true;
        self.save_settings();
        Ok(())
    }

    pub async fn ds_stop(&mut self) -> Result<()> {
        self.ds.stop().await?;
        self.settings.ds_running = false;
        self.save_settings();
        Ok(())
    }

    pub fn ds_status(&self) -> DsStatus {
        self.ds.status()
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
        let server_running = self.server_session.is_some();
        let client_running = self.client_session.is_some();
        let bridge_role = match (server_running, client_running) {
            (true, true) => Some("Server + Client".to_string()),
            (true, false) => Some("Server".to_string()),
            (false, true) => Some("Client".to_string()),
            (false, false) => None,
        };
        let servers = self.list_servers().await?;
        let interfaces = self.list_interfaces().unwrap_or_default();
        let ds = self.ds.status();
        Ok(ControllerState {
            bridge_running: server_running || client_running,
            bridge_role,
            server_running,
            client_running,
            ds,
            servers,
            interfaces,
            resume_errors: self.resume_errors.clone(),
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
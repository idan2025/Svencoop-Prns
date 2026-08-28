//! The bridge itself: builds the Prns node, wires interfaces, and pumps
//! GoldSrc UDP datagrams in and out of Reticulum Links.
//!
//! Wire contract: one raw GoldSrc UDP datagram per Reticulum link packet,
//! bytes-in-bytes-out. Reticulum supplies encryption, ordering, routing.
//!
//! Server side
//! -----------
//!  - Announces `sven-coop.server` regularly.
//!  - For each accepted link, opens a UDP socket to the local Sven Co-op
//!    server and pumps link<->UDP both ways until the link closes.
//!
//! Client side
//! -----------
//!  - Binds 127.0.0.1:27015.
//!  - Discovers `sven-coop.server` via announce (or uses `--server-hash`).
//!  - For each distinct GoldSrc client source addr, opens a link and pumps
//!    UDP<->link both ways until the link closes.
//!
//! Both sides also accumulate every `sven-coop.server` announce heard into a
//! discovered-server list (`BridgeSession::discovered`), which the GUI uses as
//! its server browser. The CLI only dials the first one it hears.
//!
//! The node's `run()` future is `!Send` (it holds a non-Send guard across an
//! await), so it cannot be `tokio::spawn`'d on a multi-thread runtime. Each
//! `BridgeSession` therefore drives the node on a dedicated current-thread
//! runtime + `LocalSet` in its own OS thread, and hands the caller a
//! `PrnsNodeHandle` (which is `Send + Sync`) for live control plus a stop
//! channel. The CLI just waits for the node to finish; the GUI holds the
//! handle, reads the discovered list, edits interfaces live, and stops it.

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use personal_rns::prelude::*;
use personal_rns::{load_or_create_identity_secret, IdentitySecretFileError};
use prns_core::engine::{SendToLink, SendToLinkPayload};
use prns_core::routing::delivery::Delivery;
use prns_core::routing::links::LinkId;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::task::LocalSet;
use tracing::{debug, error, info, warn};

use crate::config::{BridgeConfig, BridgeRole, ClientArgs, ServerArgs};
use crate::framing::{frame, Reassembler};
use crate::{SC_APP_NAME, SC_ASPECT_CLIENT, SC_ASPECT_SERVER};

const UDP_READ_BUF: usize = 8192;

pub async fn run_bridge(cfg: BridgeConfig) -> Result<()> {
    let session = match cfg {
        BridgeConfig::Server(args) => BridgeSession::start_server(args).await?,
        BridgeConfig::Client(args) => BridgeSession::start_client(args).await?,
    };
    session.await_completion().await
}

/// A running bridge: owns a `PrnsNodeHandle` (for live interface control and
/// introspection) and the discovered-server list, and can stop the node.
///
/// The node itself runs on a dedicated thread (see the module docs); this
/// struct holds the `Send` handle + a stop channel, so it is `Send` and can
/// live behind a `Mutex`/`Arc` in the GUI.
pub struct BridgeSession {
    handle: PrnsNodeHandle,
    discovered: Arc<RwLock<Vec<DiscoveredServer>>>,
    server_hash: Option<DestinationHash>,
    role: BridgeRole,
    stop_tx: Option<oneshot::Sender<()>>,
    done: Option<oneshot::Receiver<()>>,
}

/// A discovered `sven-coop.server` destination heard via announce.
#[derive(Debug, Clone, Copy)]
pub struct DiscoveredServer {
    pub destination_hash: DestinationHash,
    pub last_seen: Instant,
}

impl BridgeSession {
    /// The vendored Prns handle — exposes live interface add/remove/rename,
    /// introspection (routes, destination identities, link count), and link/
    /// path/announce commands. See `personal_rns::prelude::PrnsNodeHandle`.
    pub fn handle(&self) -> &PrnsNodeHandle {
        &self.handle
    }

    pub fn role(&self) -> BridgeRole {
        self.role
    }

    /// The host's own announced server hash (server side only).
    pub fn server_hash(&self) -> Option<DestinationHash> {
        self.server_hash
    }

    /// Snapshot of discovered `sven-coop.server` destinations (the browser list).
    pub async fn discovered(&self) -> Vec<DiscoveredServer> {
        self.discovered.read().await.clone()
    }

    /// Stop the node. Fire-and-forget: the dedicated thread tears down on its
    /// next tick (the node's `run()` future is dropped and its runtime exits,
    /// which cancels the spawned relay tasks).
    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Drive the session to completion (CLI path): wait until the node stops on
    /// its own (error or Ctrl-C). Consumes the session.
    pub async fn await_completion(mut self) -> Result<()> {
        match self.done.take() {
            Some(rx) => {
                let _ = rx.await;
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub async fn start_server(args: ServerArgs) -> Result<Self> {
        // Computed up front (identical inputs to the one built again inside
        // the spawned node thread below) so it can be threaded into
        // spawn_bridge_node's `server_hash` param and surfaced via
        // `BridgeSession::server_hash()` — previously that param was always
        // `None` here, so the server's own destination hash was computable
        // but never actually exposed anywhere outside the startup log line.
        let precomputed_hash = {
            let identity = load_identity(&args.identity)?;
            PreConfiguredDestination::Single {
                app_name: SC_APP_NAME,
                aspects: &[SC_ASPECT_SERVER],
                identity,
                announce_app_data: b"sc-rns-bridge",
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::No,
            }
            .destination_hash()
            .map_err(|e| anyhow!("invalid destination name: {e:?}"))?
        };
        spawn_bridge_node(BridgeRole::Server, Some(precomputed_hash), move |discovered| async move {
            let identity = load_identity(&args.identity)?;
            let destination = PreConfiguredDestination::Single {
                app_name: SC_APP_NAME,
                aspects: &[SC_ASPECT_SERVER],
                identity: identity.clone(),
                announce_app_data: b"sc-rns-bridge",
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::No,
            };
            let server_hash = destination
                .destination_hash()
                .map_err(|e| anyhow!("invalid destination name: {e:?}"))?;
            info!(server_hash = ?server_hash.as_bytes(), "Sven Coop bridge server starting");

            let sc_addr: SocketAddr = format!("{}:{}", args.sc_host, args.sc_port)
                .parse()
                .with_context(|| format!("invalid --sc-host/--sc-port: {}:{}", args.sc_host, args.sc_port))?;
            info!(sc_addr = %sc_addr, "bridging to Sven Coop server");

            let (event_tx, event_rx) = mpsc::unbounded_channel::<BridgeEvent>();
            let link_senders: LinkSenders = Arc::new(RwLock::new(std::collections::HashMap::new()));

            let node = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: Some(identity),
                pre_configured_destinations: [destination],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: {
                    let event_tx = event_tx.clone();
                    move |event, _state| funnel_event(event, &event_tx)
                },
                interfaces: |node: &PrnsNodeHandle| {
                    attach_interfaces(node, args.tcp.as_deref(), args.auto);
                },
                persistence: NoPersistence,
            });
            let handle = node.handle();

            // Announcer.
            let announcer = handle.clone();
            let interval = args.announce_interval;
            let _announce_task = tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(interval.max(1)));
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    if announcer
                        .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                            destination: server_hash,
                            target: AnnounceTarget::AllInterfaces,
                            app_data: AnnounceAppData::Registered,
                        }))
                        .is_none()
                    {
                        return;
                    }
                }
            });

            // Event router.
            let router_handle = handle.clone();
            let router_senders = link_senders.clone();
            let router_discovered = discovered.clone();
            let _router_task = tokio::spawn(async move {
                let mut event_rx = event_rx;
                while let Some(event) = event_rx.recv().await {
                    match event {
                        BridgeEvent::AnnounceHeard { destination } => {
                            remember_server(&router_discovered, destination).await;
                        }
                        BridgeEvent::LinkEstablished { link_id } => {
                            if router_senders.read().await.contains_key(&link_id) {
                                continue;
                            }
                            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(256);
                            router_senders.write().await.insert(link_id, tx);

                            let senders = router_senders.clone();
                            let handle = router_handle.clone();
                            tokio::spawn(async move {
                                debug!(link = ?link_id, sc = %sc_addr, "server relay started");
                                let sock = match UdpSocket::bind("0.0.0.0:0").await {
                                    Ok(s) => Arc::new(s),
                                    Err(e) => {
                                        error!(link = ?link_id, error = %e, "failed to bind relay UDP socket");
                                        senders.write().await.remove(&link_id);
                                        let _ = handle.close_link(link_id);
                                        return;
                                    }
                                };
                                if let Err(e) = sock.connect(sc_addr).await {
                                    error!(link = ?link_id, error = %e, "failed to connect relay UDP socket to SC server");
                                    senders.write().await.remove(&link_id);
                                    let _ = handle.close_link(link_id);
                                    return;
                                }
                                let sock_send = sock.clone();
                                let to_sc = tokio::spawn(async move {
                                    let mut reassembler = Reassembler::default();
                                    while let Some(chunk) = rx.recv().await {
                                        if chunk.is_empty() {
                                            break;
                                        }
                                        if let Some(datagram) = reassembler.push(&chunk) {
                                            if sock_send.send(&datagram).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                });
                                let sock_recv = sock.clone();
                                let from_sc = {
                                    let handle = handle.clone();
                                    tokio::spawn(async move {
                                        let mut buf = vec![0u8; UDP_READ_BUF];
                                        loop {
                                            match sock_recv.recv(&mut buf).await {
                                                Ok(n) if n > 0 => {
                                                    let datagram = &buf[..n];
                                                    for chunk in frame(datagram) {
                                                        let payload = SendToLinkPayload::from_slice(&chunk)
                                                            .expect("framed chunk fits link MDU");
                                                        if handle
                                                            .issue(PrnsCommand::SendToLink(SendToLink {
                                                                link_id,
                                                                payload,
                                                            }))
                                                            .is_none()
                                                        {
                                                            return;
                                                        }
                                                    }
                                                }
                                                Ok(_) => continue,
                                                Err(e) => {
                                                    debug!(link = ?link_id, error = ?e, "UDP recv from SC ended");
                                                    break;
                                                }
                                            }
                                        }
                                    })
                                };
                                let _ = tokio::join!(to_sc, from_sc);
                                senders.write().await.remove(&link_id);
                                let _ = handle.close_link(link_id);
                                debug!(link = ?link_id, "server relay ended");
                            });
                        }
                        BridgeEvent::LinkClosed { link_id } => {
                            if let Some(tx) = router_senders.write().await.remove(&link_id) {
                                let _ = tx.send(Vec::new()).await;
                            }
                        }
                        BridgeEvent::LinkData { link_id, bytes } => {
                            if let Some(tx) = router_senders.read().await.get(&link_id).cloned() {
                                if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(bytes) {
                                    debug!(link = ?link_id, "server relay channel full; dropping chunk");
                                }
                            } else {
                                debug!(link = ?link_id, "server LinkData for unknown link");
                            }
                        }
                    }
                }
            });

            info!("server node running; press Ctrl-C to stop");
            Ok((handle, async move { let _ = node.run().await; }))
        })
        .await
    }

    pub async fn start_client(args: ClientArgs) -> Result<Self> {
        spawn_bridge_node(BridgeRole::Client, None, move |discovered| async move {
            let identity = load_identity(&args.identity)?;
            let listen_addr: SocketAddr = format!("127.0.0.1:{}", args.listen_port)
                .parse()
                .with_context(|| format!("invalid --listen-port: {}", args.listen_port))?;
            info!(listen = %listen_addr, "Sven Coop bridge client starting");

            let target_hash = match args.server_hash.as_deref() {
                Some(hex) => Some(parse_destination_hash(hex).context("invalid --server-hash")?),
                None => None,
            };

            let udp = UdpSocket::bind(listen_addr)
                .await
                .with_context(|| format!("binding UDP listener on {listen_addr}"))?;
            info!(listen = %listen_addr, "GoldSrc clients should connect to this address");

            let (event_tx, event_rx) = mpsc::unbounded_channel::<BridgeEvent>();
            let destination = PreConfiguredDestination::Single {
                app_name: SC_APP_NAME,
                aspects: &[SC_ASPECT_CLIENT],
                identity: identity.clone(),
                announce_app_data: b"",
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::No,
            };

            let node = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: Some(identity),
                pre_configured_destinations: [destination],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: {
                    let event_tx = event_tx.clone();
                    move |event, _state| funnel_event(event, &event_tx)
                },
                interfaces: |node: &PrnsNodeHandle| {
                    attach_interfaces(node, args.tcp.as_deref(), args.auto);
                },
                persistence: NoPersistence,
            });
            let handle = node.handle();

            let server_target: Arc<RwLock<Option<DestinationHash>>> = Arc::new(RwLock::new(target_hash));
            if let Some(h) = target_hash {
                info!(server_hash = ?h.as_bytes(), "will connect to explicit server hash");
            } else {
                info!("no --server-hash given; waiting to discover sven-coop.server via announce");
            }

            // Proactively request a path to an explicit server hash so the first
            // game packet isn't dropped with NoRouteToDestination (announces may
            // be slow or never rebroadcast between same-interface peers).
            if let Some(h) = target_hash {
                let probe_handle = handle.clone();
                let _path_probe_task = tokio::spawn(async move {
                    for attempt in 1..=12u32 {
                        match probe_handle.request_path(h).await {
                            Ok(_) => {
                                info!(server_hash = ?h.as_bytes(), attempt, "path to server resolved via path request");
                                return;
                            }
                            Err(e) => {
                                debug!(attempt, error = ?e, "path request pending; retrying in 5s");
                                tokio::time::sleep(Duration::from_secs(5)).await;
                            }
                        }
                    }
                    warn!(server_hash = ?h.as_bytes(), "could not resolve path to server after retries");
                });
            }

            let link_data: LinkSenders = Arc::new(RwLock::new(std::collections::HashMap::new()));
            let udp_links: Arc<RwLock<std::collections::HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>>> =
                Arc::new(RwLock::new(std::collections::HashMap::new()));

            let router_target = server_target.clone();
            let router_link_data = link_data.clone();
            let router_discovered = discovered.clone();
            let _router_task = tokio::spawn(async move {
                let mut event_rx = event_rx;
                while let Some(event) = event_rx.recv().await {
                    match event {
                        BridgeEvent::AnnounceHeard { destination } => {
                            remember_server(&router_discovered, destination).await;
                            let mut t = router_target.write().await;
                            if t.is_none() {
                                info!(server_hash = ?destination.as_bytes(), "discovered Sven Coop server announce");
                                *t = Some(destination);
                            }
                        }
                        BridgeEvent::LinkEstablished { .. } => {}
                        BridgeEvent::LinkClosed { link_id } => {
                            router_link_data.write().await.remove(&link_id);
                        }
                        BridgeEvent::LinkData { link_id, bytes } => {
                            if let Some(tx) = router_link_data.read().await.get(&link_id).cloned() {
                                if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(bytes) {
                                    debug!(link = ?link_id, "client relay channel full; dropping chunk");
                                }
                            } else {
                                debug!(link = ?link_id, "client LinkData for unknown link");
                            }
                        }
                    }
                }
            });

            let udp: Arc<UdpSocket> = Arc::new(udp);
            let udp_handle = handle.clone();
            let udp_target = server_target.clone();
            let udp_link_data = link_data.clone();
            let _udp_task = tokio::spawn(async move {
                let mut buf = vec![0u8; UDP_READ_BUF];
                loop {
                    let (n, src) = match udp.recv_from(&mut buf).await {
                        Ok(p) => p,
                        Err(e) => {
                            error!(error = %e, "client UDP listener died");
                            return;
                        }
                    };
                    let pkt = buf[..n].to_vec();

                    if let Some(tx) = udp_links.read().await.get(&src).cloned() {
                        if tx.send(pkt).await.is_err() {
                            udp_links.write().await.remove(&src);
                        }
                        continue;
                    }

                    let Some(target) = *udp_target.read().await else {
                        warn!(src = %src, "first packet seen but no server discovered yet; dropping");
                        continue;
                    };

                    let handle = udp_handle.clone();
                    let udp = udp.clone();
                    let links = udp_links.clone();
                    let link_data_map = udp_link_data.clone();
                    tokio::spawn(async move {
                        debug!(src = %src, target = ?target.as_bytes(), "establishing link to server");
                        let link_id = match handle.establish_link(target).await {
                            Ok(id) => id,
                            Err(e) => {
                                info!(src = %src, error = ?e, "no route to server; requesting path then retrying");
                                match handle.request_path(target).await {
                                    Ok(_) => {}
                                    Err(pe) => {
                                        error!(src = %src, error = ?pe, "path request to server failed");
                                        return;
                                    }
                                }
                                match handle.establish_link(target).await {
                                    Ok(id) => id,
                                    Err(e2) => {
                                        error!(src = %src, error = ?e2, "establish link failed after path request");
                                        return;
                                    }
                                }
                            }
                        };
                        debug!(src = %src, link = ?link_id, "link established");

                        let (udp_to_link_tx, mut udp_to_link_rx) = mpsc::channel::<Vec<u8>>(256);
                        links.write().await.insert(src, udp_to_link_tx);

                        for chunk in frame(&pkt) {
                            let payload = SendToLinkPayload::from_slice(&chunk)
                                .expect("framed chunk fits link MDU");
                            if handle
                                .issue(PrnsCommand::SendToLink(SendToLink { link_id, payload }))
                                .is_none()
                            {
                                error!(src = %src, link = ?link_id, "first send failed: node stopped");
                                links.write().await.remove(&src);
                                let _ = handle.close_link(link_id);
                                return;
                            }
                        }

                        let (link_to_udp_tx, mut link_to_udp_rx) = mpsc::channel::<Vec<u8>>(256);
                        link_data_map.write().await.insert(link_id, link_to_udp_tx);
                        debug!(src = %src, link = ?link_id, "client relay registered");

                        let h1 = handle.clone();
                        let send_task = tokio::spawn(async move {
                            while let Some(bytes) = udp_to_link_rx.recv().await {
                                if bytes.is_empty() {
                                    break;
                                }
                                for chunk in frame(&bytes) {
                                    let payload = SendToLinkPayload::from_slice(&chunk)
                                        .expect("framed chunk fits link MDU");
                                    if h1
                                        .issue(PrnsCommand::SendToLink(SendToLink { link_id, payload }))
                                        .is_none()
                                    {
                                        return;
                                    }
                                }
                            }
                        });

                        let udp_back = udp.clone();
                        let recv_task = tokio::spawn(async move {
                            let mut reassembler = Reassembler::default();
                            while let Some(chunk) = link_to_udp_rx.recv().await {
                                if let Some(datagram) = reassembler.push(&chunk) {
                                    if udp_back.send_to(&datagram, src).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        });

                        let _ = tokio::join!(send_task, recv_task);
                        link_data_map.write().await.remove(&link_id);
                        links.write().await.remove(&src);
                        let _ = handle.close_link(link_id);
                        debug!(src = %src, link = ?link_id, "client relay ended");
                    });
                }
            });

            info!("client node running; press Ctrl-C to stop");
            Ok((handle, async move { let _ = node.run().await; }))
        })
        .await
    }
}

/// Run a bridge node on a dedicated current-thread runtime + `LocalSet` (the
/// node's `run()` future is `!Send`, so it can't be spawned on a multi-thread
/// runtime). `build` wires the node + relay tasks and returns the handle plus
/// the node's `run()` future; this helper drives that future, hands the caller
/// the handle + a stop channel, and signals `done` when the node exits.
async fn spawn_bridge_node<B, Fut, NodeRun>(
    role: BridgeRole,
    server_hash: Option<DestinationHash>,
    build: B,
) -> Result<BridgeSession>
where
    B: FnOnce(Arc<RwLock<Vec<DiscoveredServer>>>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(PrnsNodeHandle, NodeRun)>> + 'static,
    NodeRun: Future<Output = ()> + 'static,
{
    let discovered: Arc<RwLock<Vec<DiscoveredServer>>> = Arc::new(RwLock::new(Vec::new()));
    let disc_for_thread = discovered.clone();
    let (init_tx, init_rx) = oneshot::channel::<Result<(PrnsNodeHandle, oneshot::Sender<()>)>>();
    let (done_tx, done_rx) = oneshot::channel::<()>();

    std::thread::Builder::new()
        .name("sc-rns-bridge-node".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => {
                    let _ = init_tx.send(Err(anyhow!("build runtime: {e}")));
                    return;
                }
            };
            let local = LocalSet::new();
            let _ = rt.block_on(local.run_until(async move {
                let (handle, node_run) = match build(disc_for_thread).await {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                let (stop_tx, stop_rx) = oneshot::channel::<()>();
                if init_tx.send(Ok((handle, stop_tx))).is_err() {
                    return;
                }
                tokio::select! {
                    _ = node_run => {}
                    _ = stop_rx => {}
                }
                let _ = done_tx.send(());
            }));
        })
        .context("failed to spawn bridge node thread")?;

    let (handle, stop_tx) = init_rx
        .await
        .map_err(|_| anyhow!("bridge node thread died before starting"))??;
    Ok(BridgeSession {
        handle,
        discovered,
        server_hash,
        role,
        stop_tx: Some(stop_tx),
        done: Some(done_rx),
    })
}

/// Insert or refresh a discovered server in the browser list (dedup by hash).
async fn remember_server(list: &Arc<RwLock<Vec<DiscoveredServer>>>, destination: DestinationHash) {
    let mut l = list.write().await;
    if let Some(s) = l.iter_mut().find(|s| s.destination_hash == destination) {
        s.last_seen = Instant::now();
    } else {
        l.push(DiscoveredServer { destination_hash: destination, last_seen: Instant::now() });
    }
}

// =========================================================================
// Shared event routing
// =========================================================================

#[derive(Debug)]
enum BridgeEvent {
    AnnounceHeard { destination: DestinationHash },
    LinkEstablished { link_id: LinkId },
    LinkClosed { link_id: LinkId },
    LinkData { link_id: LinkId, bytes: Vec<u8> },
}

fn funnel_event(event: PrnsEvent<'_>, tx: &mpsc::UnboundedSender<BridgeEvent>) {
    match event {
        PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
            let _ = tx.send(BridgeEvent::AnnounceHeard { destination });
        }
        PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(established)) => {
            let _ = tx.send(BridgeEvent::LinkEstablished {
                link_id: established.link_id,
            });
        }
        PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, .. }) => {
            let _ = tx.send(BridgeEvent::LinkClosed { link_id });
        }
        PrnsEvent::Message(Message::Delivered(Delivery::Link(link_delivery))) => {
            let _ = tx.send(BridgeEvent::LinkData {
                link_id: link_delivery.link_id,
                bytes: link_delivery.plaintext.to_vec(),
            });
        }
        _ => {}
    }
}

type LinkSenders = Arc<RwLock<std::collections::HashMap<LinkId, mpsc::Sender<Vec<u8>>>>>;

pub fn attach_interfaces(node: &PrnsNodeHandle, tcp: Option<&str>, auto: bool) {
    info!(tcp = ?tcp, auto, "attach_interfaces called");
    if let Some(addr) = tcp {
        if let Some(colon) = addr.rfind(':') {
            let host = &addr[..colon];
            let port: u16 = addr[colon + 1..].parse().unwrap_or(0);
            // 0.0.0.0 (or empty host) means "bind a TCP server here";
            // any other host means "connect to a TCP server there".
            if host == "0.0.0.0" || host.is_empty() {
                let node = node.clone();
                let addr = addr.to_string();
                tokio::spawn(async move {
                    match TcpServer::bind(&addr).await {
                        Ok(srv) => {
                            node.supervise(srv);
                            info!(tcp = %addr, "attached TCP server interface");
                        }
                        Err(e) => error!(tcp = %addr, error = ?e, "failed to bind TCP server"),
                    }
                });
            } else if port > 0 {
                let client = TcpClientInterface::new(addr.to_string());
                node.attach(client);
                info!(tcp = %addr, "attached TCP client interface");
            }
        } else {
            warn!(tcp = ?addr, "ignoring --tcp without a port");
        }
    }
    if auto {
        node.attach(AutoWifi::default());
        info!("attached Wi-Fi/LAN auto-discovery interface");
    }
    if tcp.is_none() && !auto {
        warn!("no interfaces attached; pass --tcp <host:port> and/or --auto, or this node can't talk");
    }
}

fn parse_destination_hash(hex: &str) -> Result<DestinationHash> {
    let hex = hex.trim();
    let bytes = hex::decode(hex).map_err(|e| anyhow!("invalid hex in --server-hash: {e}"))?;
    DestinationHash::from_slice(&bytes)
        .ok_or_else(|| anyhow!("--server-hash must be 16 bytes (32 hex chars)"))
}

fn load_identity(path: &Path) -> Result<ZeroizingIdentity> {
    load_or_create_identity_secret(path)
        .map_err(|e: IdentitySecretFileError| anyhow::Error::from(e))
        .with_context(|| format!("loading identity at {}", path.display()))
}

type ZeroizingIdentity = Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>;
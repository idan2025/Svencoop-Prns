//! Loopback integration tests.
//!
//! Topology (all on localhost):
//!
//!   GoldSrc test client  --UDP-->  bridge client  --RNS/TCP-->  bridge server  --UDP-->  mock SC echo
//!          ^_________________________ | __________________________________________________|
//!                                     v
//!                              (echo flows back the same way)
//!
//! The mock SC server is a plain UDP echo socket. The test "GoldSrc client"
//! is a UDP socket that sends a datagram to the bridge client's listen port
//! and expects the echo to come back.

use std::time::Duration;

use sc_rns_bridge::config::{ClientArgs, ServerArgs};
use tokio::net::UdpSocket;

mod common;

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sc_rns_bridge=debug,personal_rns=info".into()),
        )
        .try_init();
}

/// Spin up a mock SC UDP echo server on a free port; returns its address.
async fn spawn_mock_sc_echo() -> std::net::SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            let (n, src) = match sock.recv_from(&mut buf).await {
                Ok(p) => p,
                Err(_) => return,
            };
            let _ = sock.send_to(&buf[..n], src).await;
        }
    });
    addr
}

/// Start a bridge server and client on isolated localhost TCP, bridging to
/// `sc_addr`. Returns the UDP address the test's GoldSrc client should send to.
fn start_bridge_pair(sc_addr: std::net::SocketAddr) -> (u16, std::thread::JoinHandle<()>, std::thread::JoinHandle<()>) {
    let tcp_port = common::free_tcp_port();
    let client_listen_port = common::free_udp_port();

    let dir = std::env::temp_dir().join(format!("sc-rns-test-{}-{}", std::process::id(), tcp_port));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let server_identity = dir.join("server.identity");
    let client_identity = dir.join("client.identity");

    let server_args = ServerArgs {
        sc_port: sc_addr.port(),
        sc_host: "127.0.0.1".to_string(),
        identity: server_identity,
        tcp: Some(format!("0.0.0.0:{tcp_port}")),
        auto: false,
        announce_interval: 1,
        name: None,
    };
    let server_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let _ = sc_rns_bridge::run_bridge(sc_rns_bridge::BridgeConfig::Server(server_args)).await;
        });
    });

    let client_args = ClientArgs {
        listen_port: client_listen_port,
        server_hash: None,
        identity: client_identity,
        tcp: Some(format!("127.0.0.1:{tcp_port}")),
        auto: false,
    };
    let client_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let _ = sc_rns_bridge::run_bridge(sc_rns_bridge::BridgeConfig::Client(client_args)).await;
        });
    });

    (client_listen_port, server_thread, client_thread)
}

/// Send `payload` to the bridge client and wait for the echo to come back.
/// Retries for up to `timeout` to allow announce discovery + link setup.
async fn echo_round_trip(client_listen_port: u16, payload: &[u8], timeout: Duration) -> bool {
    let test_client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen_addr: std::net::SocketAddr = format!("127.0.0.1:{client_listen_port}").parse().unwrap();
    // Give the bridge client a moment to bind its UDP listener.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = test_client.connect(listen_addr).await;

    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = vec![0u8; 8192];
    loop {
        if test_client.send(payload).await.is_err() {
            tokio::time::sleep(Duration::from_millis(300)).await;
            continue;
        }
        match tokio::time::timeout(Duration::from_millis(800), test_client.recv(&mut buf)).await {
            Ok(Ok(n)) if &buf[..n] == payload => return true,
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_echo_small_packet() {
    init_tracing();
    let sc_addr = spawn_mock_sc_echo().await;
    let (client_port, _s, _c) = start_bridge_pair(sc_addr);
    let payload = b"hello-sc-rns";
    let ok = echo_round_trip(client_port, payload, Duration::from_secs(30)).await;
    assert!(ok, "did not receive small-packet echo within timeout");
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_echo_oversized_packet() {
    init_tracing();
    let sc_addr = spawn_mock_sc_echo().await;
    let (client_port, _s, _c) = start_bridge_pair(sc_addr);
    // 1200 bytes: larger than the Reticulum link MDU (~415 bytes with the
    // default 500-byte BROADCAST_MTU), so the bridge must fragment and
    // reassemble across multiple link packets.
    let payload: Vec<u8> = (0..1200u32).map(|i| (i & 0xff) as u8).collect();
    let ok = echo_round_trip(client_port, &payload, Duration::from_secs(30)).await;
    assert!(ok, "did not receive oversized-packet echo within timeout");
}
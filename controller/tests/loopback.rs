//! Headless integration test for `sc-rns-controller`.
//!
//! Topology (all localhost), driven through the GUI's orchestrator
//! (`BridgeController`), not the raw CLI:
//!
//!   GoldSrc test client --UDP--> bridge client --RNS/TCP--> bridge server --UDP--> mock SC echo
//!          ^_____________________ | __________________________________________________|
//!                                v
//!
//! Two `BridgeController`s run side by side (one server role, one client
//! role — the prototype holds a single session per controller). We assert:
//!   1. a datagram round-trips through the whole bridge, and
//!   2. the client controller's server browser (`list_servers`) sees the
//!      server's `sven-coop.server` announce — the GUI's core value-add.
//!
//! This runs headless on .135 (no webview).

use std::time::Duration;

use sc_rns_bridge::{ClientArgs, ServerArgs};
use sc_rns_controller::BridgeController;
use tokio::net::UdpSocket;

fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sc_rns_controller=info,sc_rns_bridge=debug".into()),
        )
        .try_init();
}

/// A mock Sven Co-op server: a plain UDP echo socket on a free port.
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

#[tokio::test(flavor = "multi_thread")]
async fn controller_loopback_round_trips_and_discovers_server() {
    init_tracing();
    let sc_addr = spawn_mock_sc_echo().await;
    let tcp_port = free_tcp_port();
    let client_listen_port = free_udp_port();

    // Isolated temp dirs for each side's Reticulum identity.
    let dir = std::env::temp_dir().join(format!(
        "sc-rns-ctrl-test-{}-{}",
        std::process::id(),
        tcp_port
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let server_identity = dir.join("server.identity");
    let client_identity = dir.join("client.identity");

    // Two controllers: one hosts the bridge server, one runs the bridge client.
    let mut server_ctrl = BridgeController::new(dir.clone());
    server_ctrl
        .start_bridge_server(ServerArgs {
            sc_port: sc_addr.port(),
            sc_host: "127.0.0.1".to_string(),
            identity: server_identity,
            tcp: Some(format!("0.0.0.0:{tcp_port}")),
            auto: false,
            announce_interval: 1,
        })
        .await
        .expect("start bridge server");

    let mut client_ctrl = BridgeController::new(dir.clone());
    client_ctrl
        .start_client(ClientArgs {
            listen_port: client_listen_port,
            server_hash: None,
            identity: client_identity,
            tcp: Some(format!("127.0.0.1:{tcp_port}")),
            auto: false,
        })
        .await
        .expect("start bridge client");

    // (1) Server browser: wait for the client to hear the server's announce.
    let mut saw_server = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let servers = client_ctrl.list_servers().await.expect("list_servers");
        if !servers.is_empty() {
            // Each entry is a 32-hex destination hash.
            assert_eq!(servers[0].destination_hash.len(), 32);
            saw_server = true;
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(saw_server, "client never discovered the server via announce");

    // (2) Datagram round-trip through the whole bridge.
    let test_client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen_addr: std::net::SocketAddr =
        format!("127.0.0.1:{client_listen_port}").parse().unwrap();
    test_client.connect(listen_addr).await.unwrap();
    let payload = b"hello-sc-rns-controller";
    let mut echoed = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut buf = vec![0u8; 8192];
    loop {
        let _ = test_client.send(payload).await;
        match tokio::time::timeout(Duration::from_millis(800), test_client.recv(&mut buf)).await {
            Ok(Ok(n)) if &buf[..n] == payload => {
                echoed = true;
                break;
            }
            _ => {}
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(echoed, "did not receive echo within timeout");

    // (3) The controller's snapshot also surfaces the server + running state.
    let state = client_ctrl.state().await.expect("state");
    assert!(state.bridge_running);
    assert!(!state.servers.is_empty());

    // Tear down (the dedicated node threads exit when stop() cancels the node).
    client_ctrl.stop_client().await.expect("stop client");
    server_ctrl.stop_bridge_server().await.expect("stop server");
    let _ = std::fs::remove_dir_all(&dir);
}
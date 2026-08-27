//! Tauri v2 command shell for the Sven Co-op over Reticulum platform GUI.
//!
//! This is a thin wrapper over `sc_rns_controller::BridgeController` — every
//! `#[tauri::command]` is one user action, delegating to the controller. The
//! controller (and the bridge `BridgeSession`) is fully headless-testable;
//! this crate only exists to expose it to a web UI.
//!
//! State is a single `Arc<tokio::sync::Mutex<BridgeController>>` (tokio Mutex
//! so the guard can be held across the controller's `.await` calls). The
//! frontend polls `state()` for live updates (server browser, interfaces, DS
//! status) rather than subscribing to events — simpler and robust for a
//! prototype.
//!
//! This crate requires a webview stack (WebKit on Linux, WebView2 on Windows,
//! WKWebView on macOS) and so is NOT built on the headless .135 box — only on
//! a desktop with the webview deps installed.

use std::path::PathBuf;
use std::sync::Arc;

use sc_rns_controller::BridgeController;
use sc_rns_bridge::{ClientArgs, ServerArgs};
use serde::Serialize;
use tauri::Manager;
use tokio::sync::Mutex;

/// Shared controller state behind a tokio Mutex (held across `.await`).
type CtrlState = Arc<Mutex<BridgeController>>;

/// Convenience: lock + run a closure on the controller, stringifying errors.
async fn with_ctrl<F, T>(state: tauri::State<'_, CtrlState>, f: F) -> Result<T, String>
where
    F: FnOnce(&mut BridgeController) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send + '_>>,
    T: Send + 'static,
{
    let mut ctrl = state.lock().await;
    f(&mut ctrl).await.map_err(|e| e.to_string())
}

// ---- bridge server ----

#[tauri::command]
async fn start_bridge_server(
    state: tauri::State<'_, CtrlState>,
    bundle: tauri::State<'_, PathBuf>,
    sc_host: String,
    sc_port: u16,
    tcp: Option<String>,
    auto: bool,
    announce_interval: u64,
) -> Result<(), String> {
    let bundle_dir = bundle.inner().clone();
    with_ctrl(state, |ctrl| {
        Box::pin(async move {
            ctrl.start_bridge_server(ServerArgs {
                sc_host,
                sc_port,
                identity: bundle_dir.join("server.identity"),
                tcp,
                auto,
                announce_interval,
            })
            .await
        })
    })
    .await
}

#[tauri::command]
async fn stop_bridge_server(state: tauri::State<'_, CtrlState>) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.stop_bridge_server().await })).await
}

#[tauri::command]
async fn restart_bridge_server(state: tauri::State<'_, CtrlState>) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.restart_bridge_server().await })).await
}

// ---- bridge client ----

#[tauri::command]
async fn start_client(
    state: tauri::State<'_, CtrlState>,
    bundle: tauri::State<'_, PathBuf>,
    listen_port: u16,
    server_hash: Option<String>,
    tcp: Option<String>,
    auto: bool,
) -> Result<(), String> {
    let bundle_dir = bundle.inner().clone();
    with_ctrl(state, |ctrl| {
        Box::pin(async move {
            ctrl.start_client(ClientArgs {
                listen_port,
                server_hash,
                identity: bundle_dir.join("client.identity"),
                tcp,
                auto,
            })
            .await
        })
    })
    .await
}

#[tauri::command]
async fn stop_client(state: tauri::State<'_, CtrlState>) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.stop_client().await })).await
}

#[tauri::command]
async fn restart_client(state: tauri::State<'_, CtrlState>) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.restart_client().await })).await
}

// ---- server browser + interfaces ----

#[tauri::command]
async fn list_servers(state: tauri::State<'_, CtrlState>) -> Result<Vec<sc_rns_controller::ServerEntry>, String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.list_servers().await })).await
}

#[tauri::command]
async fn list_interfaces(state: tauri::State<'_, CtrlState>) -> Result<Vec<sc_rns_controller::InterfaceInfo>, String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.list_interfaces() })).await
}

#[tauri::command]
async fn add_interface_tcp(
    state: tauri::State<'_, CtrlState>,
    addr: String,
    ifac_name: Option<String>,
) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.add_interface_tcp(addr, ifac_name).await })).await
}

#[tauri::command]
async fn add_interface_auto(state: tauri::State<'_, CtrlState>) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.add_interface_auto() })).await
}

#[tauri::command]
async fn remove_interface(state: tauri::State<'_, CtrlState>, id: String) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.remove_interface(&id) })).await
}

#[tauri::command]
async fn rename_interface(
    state: tauri::State<'_, CtrlState>,
    id: String,
    name: String,
) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.rename_interface(&id, name) })).await
}

// ---- dedicated server ----

#[tauri::command]
async fn ds_start(
    state: tauri::State<'_, CtrlState>,
    port: u16,
    maxplayers: u32,
    map: String,
    install_dir: Option<String>,
) -> Result<(), String> {
    with_ctrl(state, |ctrl| {
        Box::pin(async move {
            ctrl.ds_start(sc_rns_controller::DsStartArgs {
                port,
                maxplayers,
                map,
                install_dir: install_dir.map(PathBuf::from),
            })
            .await
        })
    })
    .await
}

#[tauri::command]
async fn ds_stop(state: tauri::State<'_, CtrlState>) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.ds_stop().await })).await
}

#[tauri::command]
async fn ds_status(state: tauri::State<'_, CtrlState>) -> Result<sc_rns_controller::DsStatus, String> {
    with_ctrl(state, |ctrl| Box::pin(async move { Ok(ctrl.ds_status()) })).await
}

// ---- connect + launch ----

#[tauri::command]
async fn connect_and_launch(state: tauri::State<'_, CtrlState>, server_hash: String) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.connect_and_launch(server_hash).await })).await
}

// ---- snapshot ----

#[derive(Serialize)]
struct StateSnapshot {
    bridge_running: bool,
    bridge_role: Option<String>,
    ds: sc_rns_controller::DsStatus,
    servers: Vec<sc_rns_controller::ServerEntry>,
    interfaces: Vec<sc_rns_controller::InterfaceInfo>,
    #[serde(default)]
    resume_errors: Vec<String>,
}

#[tauri::command]
async fn get_state(state: tauri::State<'_, CtrlState>) -> Result<StateSnapshot, String> {
    with_ctrl(state, |ctrl| {
        Box::pin(async move {
            let s = ctrl.state().await?;
            Ok(StateSnapshot {
                bridge_running: s.bridge_running,
                bridge_role: s.bridge_role,
                ds: s.ds,
                servers: s.servers,
                interfaces: s.interfaces,
                resume_errors: s.resume_errors,
            })
        })
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Bundle dir = the app's data dir (no hardcoded absolute paths).
            let bundle_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
            std::fs::create_dir_all(&bundle_dir).ok();
            app.manage(CtrlState::new(tokio::sync::Mutex::new(
                BridgeController::new(bundle_dir.clone()),
            )));
            // Stash the bundle dir for commands that build identity paths.
            app.manage(bundle_dir);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_bridge_server,
            stop_bridge_server,
            restart_bridge_server,
            start_client,
            stop_client,
            restart_client,
            list_servers,
            list_interfaces,
            add_interface_tcp,
            add_interface_auto,
            remove_interface,
            rename_interface,
            ds_start,
            ds_stop,
            ds_status,
            connect_and_launch,
            get_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
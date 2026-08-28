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
    ifac_passphrase: Option<String>,
) -> Result<(), String> {
    with_ctrl(state, |ctrl| {
        Box::pin(async move { ctrl.add_interface_tcp(addr, ifac_name, ifac_passphrase).await })
    })
    .await
}

#[tauri::command]
async fn add_interface_auto(
    state: tauri::State<'_, CtrlState>,
    ifac_name: Option<String>,
    ifac_passphrase: Option<String>,
) -> Result<(), String> {
    with_ctrl(state, |ctrl| Box::pin(async move { ctrl.add_interface_auto(ifac_name, ifac_passphrase) })).await
}

#[tauri::command]
async fn add_interface_udp(
    state: tauri::State<'_, CtrlState>,
    local: String,
    peer: String,
    ifac_name: Option<String>,
    ifac_passphrase: Option<String>,
) -> Result<(), String> {
    with_ctrl(state, |ctrl| {
        Box::pin(async move { ctrl.add_interface_udp(local, peer, ifac_name, ifac_passphrase).await })
    })
    .await
}

#[tauri::command]
async fn add_interface_websocket(
    state: tauri::State<'_, CtrlState>,
    addr: String,
    ifac_name: Option<String>,
    ifac_passphrase: Option<String>,
) -> Result<(), String> {
    with_ctrl(state, |ctrl| {
        Box::pin(async move { ctrl.add_interface_websocket(addr, ifac_name, ifac_passphrase).await })
    })
    .await
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
    eprintln!("[sc-rns-gui] get_state called");
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
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sc_rns_controller=info,sc_rns_bridge=info,personal_rns=warn,sc_rns_gui=info".into()),
        )
        .try_init();

    tauri::Builder::default()
        .setup(|app| {
            // Bundle dir resolution — portable mode first, OS app-data fallback.
            //
            // Portable mode: a `sc-rns-data/` folder next to the executable
            // (or, for an AppImage, next to the .AppImage file via the
            // APPIMAGE env var / the resolved real path). All mutable state
            // lives there: settings.json, RNS identities, the steamcmd
            // bootstrap, and the pulled Sven Co-op dedicated server (~2.74 GB).
            // This lets the whole release ship as a self-contained archive the
            // user can drop anywhere (USB stick, Desktop, etc.) and run.
            //
            // Fallback: the platform's per-app data dir (Tauri
            // `app_data_dir`) when the executable's directory isn't writable
            // (e.g. installed system-wide under /usr/bin), so a system install
            // still works without polluting the binary directory.
            let bundle_dir = resolve_bundle_dir(app);
            std::fs::create_dir_all(&bundle_dir).ok();
            eprintln!("[sc-rns-gui] bundle dir: {}", bundle_dir.display());
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
            add_interface_udp,
            add_interface_websocket,
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

/// The portable data folder name. Next to the executable (or the .AppImage
/// file), all mutable state lives here: settings.json, RNS identities, the
/// steamcmd bootstrap, and the pulled ~2.74 GB Sven Co-op dedicated server.
const PORTABLE_DIR_NAME: &str = "sc-rns-data";

/// Resolve the bundle dir (where all mutable state lives).
///
/// Portable mode: `<exe's dir>/sc-rns-data/`. For an AppImage, the AppImage
/// file's directory (via `APPIMAGE` env var) — so the data sticks with the
/// .AppImage across "moves", not the ephemeral mount point. Falls back to the
/// OS per-app data dir (Tauri `app_data_dir`) only when the exe's directory
/// isn't writable — e.g. a system install under `/usr/bin` or `/opt`.
fn resolve_bundle_dir(app: &tauri::App) -> PathBuf {
    if let Some(portable) = portable_candidate() {
        // Use portable mode if the folder already exists, or if we can create
        // it (the exe's dir is writable). System-wide installs fail the create
        // and fall back to the OS data dir.
        if portable.exists() {
            return portable;
        }
        if std::fs::create_dir_all(&portable).is_ok() {
            // Drop a marker so the user knows where their data is.
            let marker = portable.join("PORTABLE.txt");
            if !marker.exists() {
                let _ = std::fs::write(
                    &marker,
                    "Sven Co-op over Reticulum — portable data folder.\n\
                     This folder holds settings.json, RNS identities, the\n\
                     steamcmd bootstrap, and the pulled Sven Co-op dedicated\n\
                     server (~2.74 GB). Keep it with the executable.\n",
                );
            }
            return portable;
        }
    }
    // Fallback: the platform's per-app data dir.
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default())
}

/// Find the portable data folder candidate: the directory containing the
/// running executable, or — for an AppImage — the directory containing the
/// .AppImage file (the `APPIMAGE` env var points at the real .AppImage path,
/// not the ephemeral squashfs mount).
fn portable_candidate() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;

    // AppImage: `APPIMAGE` is the path to the .AppImage file. The current_exe
    // inside an AppImage points at the AppDir's AppRun, so prefer APPIMAGE to
    // keep data next to the .AppImage the user actually sees/moves.
    if let Ok(appimage) = std::env::var("APPIMAGE") {
        if !appimage.is_empty() {
            if let Some(d) = PathBuf::from(&appimage).parent() {
                return Some(d.join(PORTABLE_DIR_NAME));
            }
        }
    }
    Some(exe_dir.join(PORTABLE_DIR_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_candidate_is_exe_sibling() {
        // Not an AppImage in this test, so the candidate is <exe_dir>/sc-rns-data.
        let exe = std::env::current_exe().unwrap();
        let candidate = portable_candidate();
        assert!(candidate.is_some());
        let c = candidate.unwrap();
        assert_eq!(c.file_name().unwrap(), PORTABLE_DIR_NAME);
        assert_eq!(c.parent().unwrap(), exe.parent().unwrap());
    }

    #[test]
    fn portable_dir_name_is_stable() {
        assert_eq!(PORTABLE_DIR_NAME, "sc-rns-data");
    }
}